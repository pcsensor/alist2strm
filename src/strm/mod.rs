pub mod path;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Semaphore;
use tracing::{error, warn};

use crate::alist::AlistClient;
use crate::config::{StrmMode, StrmTaskConfig};
use path::{is_junk_file, AlistPath};

#[derive(Debug, Clone, Default)]
pub struct TaskStats {
    pub scanned_dirs: usize,
    pub discovered_files: usize,
    pub created_strm: usize,
    pub skipped_strm: usize,
    pub deleted_strm: usize,
    pub failed_files: usize,
}

pub struct StrmEngine {
    pub task: StrmTaskConfig,
    pub alist: AlistClient,
    pub base_path: String,
}

impl StrmEngine {
    pub fn new(task: StrmTaskConfig, alist: AlistClient, base_path: String) -> Self {
        Self {
            task,
            alist,
            base_path,
        }
    }

    /// 执行全量扫描和 STRM 生成
    pub async fn execute<F>(&self, mut on_log: F) -> Result<TaskStats, String>
    where
        F: FnMut(String) + Send + 'static,
    {
        let target_dir = PathBuf::from(&self.task.target_dir);
        if let Err(e) = fs::create_dir_all(&target_dir).await {
            return Err(format!("创建目标目录失败 {:?}: {}", target_dir, e));
        }

        on_log(format!(
            "▶ 开始执行任务「{}」，源目录: {}，目标目录: {}",
            self.task.name, self.task.source_dir, self.task.target_dir
        ));

        let scanned_dirs = Arc::new(AtomicUsize::new(0));
        let discovered_files = Arc::new(AtomicUsize::new(0));
        let created_strm = Arc::new(AtomicUsize::new(0));
        let skipped_strm = Arc::new(AtomicUsize::new(0));
        let failed_files = Arc::new(AtomicUsize::new(0));

        let mut stack = vec![self.task.source_dir.clone()];
        let mut processed_local_paths: HashSet<PathBuf> = HashSet::new();
        let mut bdmv_candidates: HashMap<String, Vec<AlistPath>> = HashMap::new();

        let concurrency = self.task.concurrency.max(1);
        let sem = Arc::new(Semaphore::new(concurrency));

        let valid_exts: HashSet<String> = self
            .task
            .video_extensions
            .iter()
            .map(|s| s.to_lowercase())
            .collect();

        // 阶段 1：深度优先递归扫描远端目录
        while let Some(current_dir) = stack.pop() {
            scanned_dirs.fetch_add(1, Ordering::Relaxed);
            on_log(format!("🔍 扫描目录: {}", current_dir));

            let items = match self.alist.list_dir(&current_dir).await {
                Ok(list) => list,
                Err(err) => {
                    warn!("扫描目录 {} 失败: {}", current_dir, err);
                    on_log(format!("⚠️ 目录无法读取跳过: {} (原因: {})", current_dir, err));
                    continue;
                }
            };

            for item in items {
                let full_path = path::join_alist_path(&current_dir, &item.name);

                if is_junk_file(&full_path) {
                    continue;
                }

                if item.is_dir {
                    stack.push(full_path);
                    continue;
                }

                discovered_files.fetch_add(1, Ordering::Relaxed);

                let p = AlistPath::new(
                    self.alist.effective_public_url(),
                    &self.base_path,
                    &current_dir,
                    &item.name,
                    item.size.max(0) as u64,
                    false,
                    item.sign,
                );

                // 蓝光原盘 (BDMV) 特殊收集
                if p.is_bdmv_stream_file() {
                    if let Some(root) = p.bdmv_root_dir() {
                        bdmv_candidates.entry(root).or_default().push(p);
                        continue;
                    }
                }

                // 判断文件扩展名是否为目标视频
                let ext = p.extension_lower();
                if !valid_exts.contains(&ext) {
                    continue;
                }

                // 普通视频文件处理
                let local_path = p.local_strm_path(
                    &self.task.source_dir,
                    &target_dir,
                    self.task.flatten,
                    false,
                );
                processed_local_paths.insert(local_path.clone());

                // 增量检查
                if !self.task.overwrite && local_path.exists() {
                    skipped_strm.fetch_add(1, Ordering::Relaxed);
                    continue;
                }

                let _permit = sem.clone().acquire_owned().await.unwrap();
                let alist_clone = self.alist.clone();
                let task_mode = self.task.mode;
                let c_created = created_strm.clone();
                let c_failed = failed_files.clone();

                tokio::spawn(async move {
                    let write_res = write_strm_file(&p, &local_path, task_mode, &alist_clone).await;
                    match write_res {
                        Ok(()) => {
                            c_created.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            c_failed.fetch_add(1, Ordering::Relaxed);
                            error!("写入 STRM 失败 {:?}: {}", local_path, e);
                        }
                    }
                    drop(_permit);
                });
            }
        }

        // 等待当前所有并发写文件结束
        let _ = sem.acquire_many(concurrency as u32).await;

        // 阶段 2：处理 BDMV 蓝光原盘（每个 BDMV 原盘选取体积最大的主片）
        if !bdmv_candidates.is_empty() {
            on_log(format!(
                "💿 发现 {} 个 BDMV 原盘，开始为主片生成 STRM...",
                bdmv_candidates.len()
            ));

            for (root, files) in bdmv_candidates {
                // 选最大体积的 m2ts
                if let Some(main_movie) = files.into_iter().max_by_key(|f| f.size) {
                    let local_path = main_movie.local_strm_path(
                        &self.task.source_dir,
                        &target_dir,
                        self.task.flatten,
                        true,
                    );
                    processed_local_paths.insert(local_path.clone());

                    if !self.task.overwrite && local_path.exists() {
                        skipped_strm.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }

                    match write_strm_file(
                        &main_movie,
                        &local_path,
                        self.task.mode,
                        &self.alist,
                    )
                    .await
                    {
                        Ok(()) => {
                            created_strm.fetch_add(1, Ordering::Relaxed);
                            on_log(format!("✓ 原盘主片完成: {:?}", local_path.file_name().unwrap_or_default()));
                        }
                        Err(e) => {
                            failed_files.fetch_add(1, Ordering::Relaxed);
                            on_log(format!("❌ 原盘写入失败 ({}): {}", root, e));
                        }
                    }
                }
            }
        }

        // 阶段 3：同步删除（清理本地存在但云端已被删除的孤立 .strm）
        let mut deleted_strm = 0;
        if self.task.sync_delete {
            on_log("🧹 正在检查并清理本地孤立 STRM 文件...".into());
            let local_strm_files = scan_all_local_strm(&target_dir).await;
            for local_file in local_strm_files {
                if !processed_local_paths.contains(&local_file) {
                    if let Ok(()) = fs::remove_file(&local_file).await {
                        deleted_strm += 1;
                    }
                }
            }
            if deleted_strm > 0 {
                on_log(format!("🧹 已清理 {} 个已失效的本地 STRM 文件", deleted_strm));
            }
        }

        let stats = TaskStats {
            scanned_dirs: scanned_dirs.load(Ordering::Relaxed),
            discovered_files: discovered_files.load(Ordering::Relaxed),
            created_strm: created_strm.load(Ordering::Relaxed),
            skipped_strm: skipped_strm.load(Ordering::Relaxed),
            deleted_strm,
            failed_files: failed_files.load(Ordering::Relaxed),
        };

        on_log(format!(
            "🎉 任务完成！扫描目录: {}，发现文件: {}，生成: {}，跳过: {}，删除: {}，失败: {}",
            stats.scanned_dirs,
            stats.discovered_files,
            stats.created_strm,
            stats.skipped_strm,
            stats.deleted_strm,
            stats.failed_files
        ));

        Ok(stats)
    }
}

async fn write_strm_file(
    p: &AlistPath,
    target_path: &Path,
    mode: StrmMode,
    client: &AlistClient,
) -> Result<(), String> {
    let content = match mode {
        StrmMode::AlistURL => p.alist_play_url(),
        StrmMode::RawURL => {
            let detail = client.get_fs(&p.full_path).await?;
            detail
                .raw_url
                .ok_or_else(|| "未能获取 raw_url".to_string())?
        }
    };

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建父目录失败: {}", e))?;
    }

    fs::write(target_path, content.trim())
        .await
        .map_err(|e| format!("写入文件失败: {}", e))?;

    Ok(())
}

async fn scan_all_local_strm(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let mut reader = match fs::read_dir(&dir).await {
            Ok(r) => r,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = reader.next_entry().await {
            let path = entry.path();
            if let Ok(meta) = entry.metadata().await {
                if meta.is_dir() {
                    stack.push(path);
                } else if meta.is_file() {
                    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                        if ext.eq_ignore_ascii_case("strm") {
                            result.push(path);
                        }
                    }
                }
            }
        }
    }

    result
}
