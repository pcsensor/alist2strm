use chrono::{Local, NaiveDateTime, NaiveTime, TimeZone};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};
use tracing::{info, warn};

/// 每个任务保留的最大行数
const MAX_ROWS_PER_TASK: i64 = 2000;
/// 全库保留的最大行数
const MAX_ROWS_TOTAL: i64 = 200_000;
/// 日志保留时长
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// 后台清理间隔
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(6 * 3600);

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS task_logs (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT    NOT NULL,
    ts      INTEGER NOT NULL,
    kind    TEXT    NOT NULL DEFAULT 'log',
    message TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_logs_task ON task_logs(task_id, id);
CREATE INDEX IF NOT EXISTS idx_task_logs_ts   ON task_logs(ts);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskLogEntry {
    pub id: i64,
    pub task_id: String,
    /// 本地时间，格式 `YYYY-MM-DD HH:MM:SS`
    pub time: String,
    /// `log` 或 `run_start`
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQueryResult {
    pub logs: Vec<TaskLogEntry>,
    pub max_id: i64,
    pub truncated: bool,
}

/// 基于 SQLite 的任务日志存储，数据库文件放在 `<config_dir>/task_logs.db`。
#[derive(Clone)]
pub struct TaskLogStore {
    conn: Arc<Mutex<Connection>>,
    db_path: Arc<PathBuf>,
}

impl TaskLogStore {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        if let Some(parent) = db_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                warn!("创建日志数据库目录失败 {:?}: {}", parent, e);
            }
        }

        let conn = match open_connection(&db_path) {
            Ok(conn) => conn,
            Err(e) => {
                warn!(
                    "打开日志数据库 {:?} 失败: {}，回退到内存数据库（重启后日志会丢失）",
                    db_path, e
                );
                let memory = Connection::open_in_memory().expect("打开内存数据库失败");
                let _ = memory.execute_batch(SCHEMA);
                memory
            }
        };

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: Arc::new(db_path),
        };
        info!("任务日志数据库: {:?}", store.db_path);
        store.migrate_legacy_files();
        store
    }

    /// 追加一条日志，返回 `(id, 格式化时间)`
    pub fn append(&self, task_id: &str, kind: &str, message: &str) -> Option<(i64, String)> {
        self.insert(task_id, Local::now().timestamp(), kind, message)
    }

    /// 任务开始运行时写入分隔标记
    pub fn begin_run(&self, task_id: &str) -> Option<(i64, String)> {
        self.append(task_id, "run_start", "开始运行")
    }

    pub fn read_task(&self, task_id: &str, limit: usize) -> LogQueryResult {
        let limit = limit.clamp(1, 2000) as i64;
        let conn = self.lock();

        let mut stmt = match conn.prepare(
            "SELECT id, task_id, ts, kind, message FROM task_logs \
             WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2",
        ) {
            Ok(stmt) => stmt,
            Err(e) => {
                warn!("查询任务日志失败: {}", e);
                return empty_result();
            }
        };

        let rows = stmt.query_map(params![task_id, limit + 1], map_row);
        collect_entries(rows, limit as usize)
    }

    pub fn read_all(&self, limit: usize) -> LogQueryResult {
        let limit = limit.clamp(1, 2000) as i64;
        let conn = self.lock();

        let mut stmt = match conn.prepare(
            "SELECT id, task_id, ts, kind, message FROM task_logs \
             ORDER BY id DESC LIMIT ?1",
        ) {
            Ok(stmt) => stmt,
            Err(e) => {
                warn!("查询全部任务日志失败: {}", e);
                return empty_result();
            }
        };

        let rows = stmt.query_map(params![limit + 1], map_row);
        collect_entries(rows, limit as usize)
    }

    pub fn clear_task(&self, task_id: &str) -> usize {
        let conn = self.lock();
        match conn.execute("DELETE FROM task_logs WHERE task_id = ?1", params![task_id]) {
            Ok(n) => n,
            Err(e) => {
                warn!("清空任务日志失败: {}", e);
                0
            }
        }
    }

    pub fn clear_all(&self) -> usize {
        let conn = self.lock();
        match conn.execute("DELETE FROM task_logs", []) {
            Ok(n) => n,
            Err(e) => {
                warn!("清空全部任务日志失败: {}", e);
                0
            }
        }
    }

    /// 按时间、单任务行数、全库行数清理，并尝试回收 WAL 空间
    pub fn cleanup(&self) {
        let conn = self.lock();
        let cutoff = Local::now().timestamp() - MAX_AGE.as_secs() as i64;

        if let Err(e) = conn.execute("DELETE FROM task_logs WHERE ts < ?1", params![cutoff]) {
            warn!("清理过期日志失败: {}", e);
        }

        let task_ids: Vec<String> = match conn.prepare("SELECT DISTINCT task_id FROM task_logs") {
            Ok(mut stmt) => stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map(|rows| rows.filter_map(Result::ok).collect())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        for task_id in task_ids {
            let _ = conn.execute(
                "DELETE FROM task_logs WHERE task_id = ?1 AND id NOT IN (\
                    SELECT id FROM task_logs WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2\
                 )",
                params![task_id, MAX_ROWS_PER_TASK],
            );
        }

        let _ = conn.execute(
            "DELETE FROM task_logs WHERE id NOT IN (\
                SELECT id FROM task_logs ORDER BY id DESC LIMIT ?1\
             )",
            params![MAX_ROWS_TOTAL],
        );

        let _: Result<(), _> = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }

    fn insert(&self, task_id: &str, ts: i64, kind: &str, message: &str) -> Option<(i64, String)> {
        let conn = self.lock();
        match conn.execute(
            "INSERT INTO task_logs (task_id, ts, kind, message) VALUES (?1, ?2, ?3, ?4)",
            params![task_id, ts, kind, message],
        ) {
            Ok(_) => {
                let id = conn.last_insert_rowid();
                Some((id, format_time(ts)))
            }
            Err(e) => {
                warn!("写入任务日志失败: {}", e);
                None
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn get_meta(&self, key: &str) -> Option<String> {
        self.lock()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    fn set_meta(&self, key: &str, value: &str) {
        let _ = self.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    /// 把旧版本 `logs/<task_id>.log` 文件导入数据库，成功后把目录改名，避免重复导入
    fn migrate_legacy_files(&self) {
        let dir = match self.db_path.parent() {
            Some(parent) => parent.join("logs"),
            None => return,
        };
        if !dir.is_dir() || self.get_meta("legacy_files_imported").is_some() {
            return;
        }

        let mut imported = 0usize;
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("log") {
                    continue;
                }

                let task_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();

                let fallback_ts = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or_else(|| Local::now().timestamp());

                let content = match fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(e) => {
                        warn!("读取旧日志文件 {:?} 失败: {}", path, e);
                        continue;
                    }
                };

                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (ts, kind, message) = parse_legacy_line(line, fallback_ts);
                    if self.insert(&task_id, ts, kind, &message).is_some() {
                        imported += 1;
                    }
                }
            }
        }

        if let Some(parent) = dir.parent() {
            let backup = parent.join(format!(
                "logs.imported-{}",
                Local::now().format("%Y%m%d%H%M%S")
            ));
            if let Err(e) = fs::rename(&dir, &backup) {
                warn!("重命名旧日志目录失败: {}", e);
            }
        }
        self.set_meta("legacy_files_imported", "1");

        if imported > 0 {
            info!("已从旧日志文件迁移 {} 条记录到 SQLite", imported);
        }
    }
}

fn open_connection(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    let _: Option<String> = conn
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .optional()?;
    conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskLogEntry> {
    let ts: i64 = row.get(2)?;
    Ok(TaskLogEntry {
        id: row.get(0)?,
        task_id: row.get(1)?,
        time: format_time(ts),
        kind: row.get(3)?,
        message: row.get(4)?,
    })
}

fn collect_entries<F>(
    rows: rusqlite::Result<rusqlite::MappedRows<'_, F>>,
    limit: usize,
) -> LogQueryResult
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<TaskLogEntry>,
{
    let mut entries: Vec<TaskLogEntry> = match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(e) => {
            warn!("读取任务日志失败: {}", e);
            Vec::new()
        }
    };

    let truncated = entries.len() > limit;
    entries.truncate(limit);
    let max_id = entries.first().map(|e| e.id).unwrap_or(0);
    entries.reverse();

    LogQueryResult {
        logs: entries,
        max_id,
        truncated,
    }
}

fn empty_result() -> LogQueryResult {
    LogQueryResult {
        logs: Vec::new(),
        max_id: 0,
        truncated: false,
    }
}

fn format_time(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

fn parse_legacy_line(line: &str, fallback_ts: i64) -> (i64, &'static str, String) {
    if line.contains("开始运行") {
        let ts = parse_datetime_in_text(line).unwrap_or(fallback_ts);
        return (ts, "run_start", "开始运行".to_string());
    }

    if let Some(rest) = line.strip_prefix('[') {
        if let Some((time, message)) = rest.split_once("] ") {
            if let Some(ts) = parse_timestamp(time, fallback_ts) {
                return (ts, "log", message.to_string());
            }
        }
    }

    (fallback_ts, "log", line.to_string())
}

fn parse_timestamp(value: &str, fallback_ts: i64) -> Option<i64> {
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return dt
            .and_local_timezone(Local)
            .single()
            .map(|dt| dt.timestamp());
    }

    let time = NaiveTime::parse_from_str(value, "%H:%M:%S").ok()?;
    let base = Local.timestamp_opt(fallback_ts, 0).single()?;
    base.date_naive()
        .and_time(time)
        .and_local_timezone(Local)
        .single()
        .map(|dt| dt.timestamp())
}

fn parse_datetime_in_text(line: &str) -> Option<i64> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    for pair in parts.windows(2) {
        let candidate = format!("{} {}", pair[0], pair[1]);
        if let Ok(dt) = NaiveDateTime::parse_from_str(&candidate, "%Y-%m-%d %H:%M:%S") {
            if let Some(dt) = dt.and_local_timezone(Local).single() {
                return Some(dt.timestamp());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn store() -> (TaskLogStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let store = TaskLogStore::new(dir.path().join("task_logs.db"));
        (store, dir)
    }

    #[test]
    fn append_and_read_task_logs() {
        let (store, _dir) = store();
        store.begin_run("task-1");
        let (id1, _) = store.append("task-1", "log", "扫描目录 /a").unwrap();
        let (id2, _) = store.append("task-1", "log", "生成 1.strm").unwrap();

        let result = store.read_task("task-1", 10);
        assert_eq!(result.logs.len(), 3);
        assert_eq!(result.max_id, id2);
        assert!(id2 > id1);
        assert_eq!(result.logs[0].kind, "run_start");
        assert_eq!(result.logs[1].message, "扫描目录 /a");
        assert_eq!(result.logs[2].id, id2);
        assert!(!result.truncated);
    }

    #[test]
    fn read_all_merges_tasks_in_id_order() {
        let (store, _dir) = store();
        store.append("task-a", "log", "a1");
        store.append("task-b", "log", "b1");
        store.append("task-a", "log", "a2");

        let result = store.read_all(10);
        let messages: Vec<&str> = result.logs.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(messages, vec!["a1", "b1", "a2"]);
        assert_eq!(result.logs[2].task_id, "task-a");
    }

    #[test]
    fn limit_returns_latest_rows_and_truncated_flag() {
        let (store, _dir) = store();
        for i in 0..30 {
            store.append("task-1", "log", &format!("line-{}", i));
        }

        let result = store.read_task("task-1", 10);
        assert!(result.truncated);
        assert_eq!(result.logs.len(), 10);
        assert_eq!(result.logs[0].message, "line-20");
        assert_eq!(result.logs[9].message, "line-29");
    }

    #[test]
    fn clear_task_keeps_other_tasks() {
        let (store, _dir) = store();
        store.append("task-1", "log", "a");
        store.append("task-2", "log", "b");

        store.clear_task("task-1");
        assert!(store.read_task("task-1", 10).logs.is_empty());
        assert_eq!(store.read_task("task-2", 10).logs.len(), 1);

        store.clear_all();
        assert!(store.read_task("task-2", 10).logs.is_empty());
    }

    #[test]
    fn cleanup_caps_rows_per_task() {
        let (store, _dir) = store();
        let now = Local::now().timestamp();
        for i in 0..(MAX_ROWS_PER_TASK + 10) {
            store.insert("task-1", now, "log", &format!("line-{}", i));
        }

        store.cleanup();
        let result = store.read_task("task-1", 2000);
        assert_eq!(result.logs.len(), MAX_ROWS_PER_TASK as usize);
        assert_eq!(result.logs.last().unwrap().message, "line-2009");
    }

    #[test]
    fn migrates_legacy_log_files() {
        let dir = tempdir().unwrap();
        let logs = dir.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("task-1.log"),
            "[2026-10-20 10:00:00] 扫描目录\n──── 2026-10-20 10:00:01 开始运行 ────\n",
        )
        .unwrap();

        let store = TaskLogStore::new(dir.path().join("task_logs.db"));
        let result = store.read_task("task-1", 10);

        assert_eq!(result.logs.len(), 2);
        assert_eq!(result.logs[0].kind, "log");
        assert_eq!(result.logs[0].message, "扫描目录");
        assert_eq!(result.logs[1].kind, "run_start");
        assert_eq!(result.logs[0].time, "2026-10-20 10:00:00");
        assert!(!logs.exists(), "旧目录应当被改名");
    }
}
