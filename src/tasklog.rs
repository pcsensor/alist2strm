use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tracing::warn;

/// 单个日志文件上限，超过后在下次运行开始时轮转为 `.log.1`
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// 日志保留时长
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// logs 目录总大小上限
const MAX_DIR_BYTES: u64 = 100 * 1024 * 1024;
/// 后台清理间隔
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// 单次读取日志的最大字节数，避免大文件把内存打满
const MAX_READ_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskLogEntry {
    pub time: String,
    pub message: String,
}

/// 按任务持久化日志到 `<config_dir>/logs/<task_id>.log`。
///
/// 写入量很小（一次运行通常几十到几百行），因此这里用同步文件 IO + 全局锁，
/// 保证同一行不会被并发扫描任务截断；锁只覆盖一次 append，开销可忽略。
#[derive(Clone)]
pub struct TaskLogStore {
    dir: Arc<PathBuf>,
    write_lock: Arc<Mutex<()>>,
}

impl TaskLogStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        if let Err(e) = fs::create_dir_all(&dir) {
            warn!("创建日志目录失败 {:?}: {}", dir, e);
        }
        Self {
            dir: Arc::new(dir),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn dir(&self) -> &Path {
        self.dir.as_ref()
    }

    fn path_for(&self, task_id: &str) -> PathBuf {
        self.dir.join(format!("{}.log", sanitize_id(task_id)))
    }

    fn rotate_path_for(&self, task_id: &str) -> PathBuf {
        self.dir.join(format!("{}.log.1", sanitize_id(task_id)))
    }

    /// 任务开始运行时调用：必要时轮转文件，并写入一条分隔线。
    pub fn begin_run(&self, task_id: &str) {
        let _guard = self.lock();
        self.rotate_if_needed(task_id);
        let line = format!(
            "──── {} 开始运行 ────",
            Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        self.append_locked(task_id, &line);
    }

    /// 追加一行日志，格式：`[HH:MM:SS] message`
    pub fn append(&self, task_id: &str, time: &str, message: &str) {
        let _guard = self.lock();
        self.append_locked(task_id, &format!("[{}] {}", time, message));
    }

    /// 读取文件尾部最多 `limit` 行，返回 `(entries, truncated)`
    pub fn read_tail(&self, task_id: &str, limit: usize) -> (Vec<TaskLogEntry>, bool) {
        let limit = limit.clamp(1, 2000);
        let path = self.path_for(task_id);

        let mut file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => return (Vec::new(), false),
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len == 0 {
            return (Vec::new(), false);
        }

        const CHUNK: u64 = 8192;
        let mut pos = len;
        let mut buf: Vec<u8> = Vec::new();
        let mut read_bytes: u64 = 0;

        while pos > 0 && read_bytes < MAX_READ_BYTES {
            let start = pos.saturating_sub(CHUNK);
            let n = (pos - start) as usize;
            let mut chunk = vec![0u8; n];
            if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut chunk).is_err() {
                break;
            }
            chunk.extend_from_slice(&buf);
            buf = chunk;
            pos = start;
            read_bytes += n as u64;

            if buf.iter().filter(|b| **b == b'\n').count() > limit {
                break;
            }
        }

        let text = String::from_utf8_lossy(&buf);
        let mut lines: Vec<&str> = text.split('\n').collect();
        if lines.last() == Some(&"") {
            lines.pop();
        }
        // 从文件中间开始读时，第一行大概率是被截断的半行，丢弃
        if pos > 0 && !lines.is_empty() {
            lines.remove(0);
        }

        let truncated = pos > 0 || lines.len() > limit;
        let start = lines.len().saturating_sub(limit);
        let entries = lines[start..].iter().map(|l| parse_line(l)).collect();
        (entries, truncated)
    }

    /// 清空单个任务日志（含轮转备份）
    pub fn clear(&self, task_id: &str) {
        for path in [self.path_for(task_id), self.rotate_path_for(task_id)] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warn!("删除日志文件 {:?} 失败: {}", path, e),
            }
        }
    }

    /// 清空所有任务日志
    pub fn clear_all(&self) {
        let entries = match fs::read_dir(self.dir()) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && is_log_file(&path) {
                if let Err(e) = fs::remove_file(&path) {
                    warn!("删除日志文件 {:?} 失败: {}", path, e);
                }
            }
        }
    }

    /// 按保留时长和目录总量清理过期日志
    pub fn cleanup(&self) {
        let entries = match fs::read_dir(self.dir()) {
            Ok(e) => e,
            Err(_) => return,
        };

        let now = SystemTime::now();
        let mut survivors: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
        let mut total: u64 = 0;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_log_file(&path) {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let modified = meta.modified().unwrap_or(now);
            let age = now.duration_since(modified).unwrap_or_default();

            if age > MAX_AGE {
                let _ = fs::remove_file(&path);
                continue;
            }
            total += meta.len();
            survivors.push((path, modified, meta.len()));
        }

        if total <= MAX_DIR_BYTES {
            return;
        }

        // 超出目录上限时，按修改时间从旧到新删除
        survivors.sort_by_key(|(_, modified, _)| *modified);
        for (path, _, size) in survivors {
            if total <= MAX_DIR_BYTES {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn rotate_if_needed(&self, task_id: &str) {
        let path = self.path_for(task_id);
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size < MAX_FILE_BYTES {
            return;
        }

        let rotated = self.rotate_path_for(task_id);
        let _ = fs::remove_file(&rotated);
        if let Err(e) = fs::rename(&path, &rotated) {
            warn!("轮转日志文件 {:?} 失败: {}", path, e);
        }
    }

    fn append_locked(&self, task_id: &str, line: &str) {
        let path = self.path_for(task_id);
        let mut line = line.to_string();
        if !line.ends_with('\n') {
            line.push('\n');
        }

        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(line.as_bytes()));

        if let Err(e) = result {
            warn!("写入任务日志 {:?} 失败: {}", path, e);
        }
    }
}

/// 防止 task_id 里的特殊字符逃出日志目录
fn sanitize_id(task_id: &str) -> String {
    let sanitized: String = task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn is_log_file(path: &Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name.ends_with(".log") || name.ends_with(".log.1"),
        None => false,
    }
}

fn parse_line(line: &str) -> TaskLogEntry {
    if let Some(rest) = line.strip_prefix('[') {
        if let Some((time, message)) = rest.split_once("] ") {
            return TaskLogEntry {
                time: time.to_string(),
                message: message.to_string(),
            };
        }
    }
    TaskLogEntry {
        time: String::new(),
        message: line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store() -> (TaskLogStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = TaskLogStore::new(dir.path());
        (store, dir)
    }

    #[test]
    fn append_and_read_tail() {
        let (store, _dir) = store();
        store.begin_run("task-1");
        store.append("task-1", "12:00:00", "扫描目录 /a");
        store.append("task-1", "12:00:01", "生成 1.strm");

        let (entries, truncated) = store.read_tail("task-1", 10);
        assert!(!truncated);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].time.is_empty(), "分隔线没有时间前缀");
        assert_eq!(entries[1].time, "12:00:00");
        assert_eq!(entries[1].message, "扫描目录 /a");
        assert_eq!(entries[2].message, "生成 1.strm");
    }

    #[test]
    fn only_returns_last_n_lines() {
        let (store, _dir) = store();
        for i in 0..100 {
            store.append("task-1", "12:00:00", &format!("line-{}", i));
        }

        let (entries, truncated) = store.read_tail("task-1", 10);
        assert!(truncated);
        assert_eq!(entries.len(), 10);
        assert_eq!(entries[0].message, "line-90");
        assert_eq!(entries[9].message, "line-99");
    }

    #[test]
    fn clear_removes_only_target_task() {
        let (store, _dir) = store();
        store.append("task-1", "12:00:00", "a");
        store.append("task-2", "12:00:00", "b");

        store.clear("task-1");
        assert!(store.read_tail("task-1", 10).0.is_empty());
        assert_eq!(store.read_tail("task-2", 10).0.len(), 1);

        store.clear_all();
        assert!(store.read_tail("task-2", 10).0.is_empty());
    }

    #[test]
    fn task_id_cannot_escape_log_dir() {
        let (store, dir) = store();
        store.append("../../evil", "12:00:00", "x");

        let escaped = dir
            .path()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("evil.log");
        assert!(!escaped.exists());
        assert!(dir.path().read_dir().unwrap().next().is_some());
    }
}
