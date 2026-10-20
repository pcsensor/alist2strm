use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info, warn};

use crate::alist::AlistClient;
use crate::config::{ConfigStore, StrmTaskConfig};
use crate::strm::{StrmEngine, TaskStats};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskRunStatus {
    Idle,
    Running,
    Success,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRuntimeInfo {
    pub task_id: String,
    pub status: TaskRunStatus,
    pub last_run: Option<DateTime<Local>>,
    pub last_duration_secs: Option<f64>,
    pub last_stats: Option<SerializableTaskStats>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableTaskStats {
    pub scanned_dirs: usize,
    pub discovered_files: usize,
    pub created_strm: usize,
    pub skipped_strm: usize,
    pub deleted_strm: usize,
    pub failed_files: usize,
}

impl From<TaskStats> for SerializableTaskStats {
    fn from(s: TaskStats) -> Self {
        Self {
            scanned_dirs: s.scanned_dirs,
            discovered_files: s.discovered_files,
            created_strm: s.created_strm,
            skipped_strm: s.skipped_strm,
            deleted_strm: s.deleted_strm,
            failed_files: s.failed_files,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TaskEvent {
    Log {
        task_id: String,
        time: String,
        message: String,
    },
    StatusChanged {
        task_id: String,
        status: TaskRunStatus,
        last_stats: Option<SerializableTaskStats>,
        error: Option<String>,
    },
}

#[derive(Clone)]
pub struct TaskManager {
    config_store: ConfigStore,
    runtime_states: Arc<RwLock<HashMap<String, TaskRuntimeInfo>>>,
    event_sender: broadcast::Sender<TaskEvent>,
    scheduler: Arc<RwLock<Option<JobScheduler>>>,
}

impl TaskManager {
    pub fn new(config_store: ConfigStore) -> Self {
        let (tx, _) = broadcast::channel(1000);
        Self {
            config_store,
            runtime_states: Arc::new(RwLock::new(HashMap::new())),
            event_sender: tx,
            scheduler: Arc::new(RwLock::new(None)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.event_sender.subscribe()
    }

    /// 获取所有任务的运行时状态列表
    pub async fn get_all_status(&self) -> Vec<TaskRuntimeInfo> {
        let cfg = self.config_store.get().await;
        let lock = self.runtime_states.read().await;

        cfg.tasks
            .into_iter()
            .map(|t| {
                lock.get(&t.id).cloned().unwrap_or(TaskRuntimeInfo {
                    task_id: t.id,
                    status: TaskRunStatus::Idle,
                    last_run: None,
                    last_duration_secs: None,
                    last_stats: None,
                    last_error: None,
                })
            })
            .collect()
    }

    /// 手动或定时触发执行任务
    pub async fn trigger_task(self: Arc<Self>, task_id: String) -> Result<(), String> {
        let cfg = self.config_store.get().await;
        let task_cfg = cfg
            .tasks
            .into_iter()
            .find(|t| t.id == task_id)
            .ok_or_else(|| format!("任务不存在: {}", task_id))?;

        // 检查运行状态，防止重复并发执行
        {
            let mut lock = self.runtime_states.write().await;
            let info = lock.entry(task_id.clone()).or_insert_with(|| TaskRuntimeInfo {
                task_id: task_id.clone(),
                status: TaskRunStatus::Idle,
                last_run: None,
                last_duration_secs: None,
                last_stats: None,
                last_error: None,
            });

            if info.status == TaskRunStatus::Running {
                return Err("该任务已经在运行中，请勿重复触发".to_string());
            }

            info.status = TaskRunStatus::Running;
            info.last_error = None;
        }

        // 发送状态变更广播
        let _ = self.event_sender.send(TaskEvent::StatusChanged {
            task_id: task_id.clone(),
            status: TaskRunStatus::Running,
            last_stats: None,
            error: None,
        });

        let this = self.clone();
        tokio::spawn(async move {
            this.run_internal(task_cfg, cfg.alist).await;
        });

        Ok(())
    }

    async fn run_internal(
        self: Arc<Self>,
        task_cfg: StrmTaskConfig,
        alist_cfg: crate::config::AlistConfig,
    ) {
        let task_id = task_cfg.id.clone();
        let start_time = Instant::now();
        let now = Local::now();

        let tx = self.event_sender.clone();
        let tid_for_log = task_id.clone();

        let log_cb = move |msg: String| {
            let time_str = Local::now().format("%H:%M:%S").to_string();
            let _ = tx.send(TaskEvent::Log {
                task_id: tid_for_log.clone(),
                time: time_str,
                message: msg,
            });
        };

        let result = async {
            let client = AlistClient::new(
                alist_cfg.base_url,
                alist_cfg.public_url,
                alist_cfg.token,
                alist_cfg.wait_time,
            )?;

            // 校验连接并获取 BasePath
            let me = client.test_connection().await?;
            let engine = StrmEngine::new(task_cfg.clone(), client, me.base_path);
            engine.execute(log_cb).await
        }
        .await;

        let duration = start_time.elapsed().as_secs_f64();

        let mut lock = self.runtime_states.write().await;
        let info = lock.entry(task_id.clone()).or_insert_with(|| TaskRuntimeInfo {
            task_id: task_id.clone(),
            status: TaskRunStatus::Idle,
            last_run: None,
            last_duration_secs: None,
            last_stats: None,
            last_error: None,
        });

        info.last_run = Some(now);
        info.last_duration_secs = Some(duration);

        match result {
            Ok(stats) => {
                let ser_stats = SerializableTaskStats::from(stats);
                info.status = TaskRunStatus::Success;
                info.last_stats = Some(ser_stats.clone());
                info.last_error = None;

                let _ = self.event_sender.send(TaskEvent::StatusChanged {
                    task_id,
                    status: TaskRunStatus::Success,
                    last_stats: Some(ser_stats),
                    error: None,
                });
            }
            Err(err) => {
                error!("任务 {} 执行出错: {}", task_id, err);
                let time_str = Local::now().format("%H:%M:%S").to_string();
                let _ = self.event_sender.send(TaskEvent::Log {
                    task_id: task_id.clone(),
                    time: time_str,
                    message: format!("❌ 任务失败: {}", err),
                });

                info.status = TaskRunStatus::Failed;
                info.last_error = Some(err.clone());

                let _ = self.event_sender.send(TaskEvent::StatusChanged {
                    task_id,
                    status: TaskRunStatus::Failed,
                    last_stats: None,
                    error: Some(err),
                });
            }
        }
    }

    /// 重新加载并启动 Cron 定时调度器
    pub async fn reload_scheduler(self: &Arc<Self>) -> Result<(), String> {
        let mut sched_lock = self.scheduler.write().await;
        if let Some(mut existing) = sched_lock.take() {
            let _ = existing.shutdown().await;
        }

        let sched = JobScheduler::new()
            .await
            .map_err(|e| format!("创建调度器失败: {}", e))?;

        let app_cfg = self.config_store.get().await;

        for task in app_cfg.tasks {
            if let Some(cron_str) = task.cron.filter(|s| !s.trim().is_empty()) {
                let this = self.clone();
                let tid = task.id.clone();
                let job_cron = cron_str.clone();

                match Job::new_async(cron_str.as_str(), move |_uuid, _lock| {
                    let this = this.clone();
                    let tid = tid.clone();
                    Box::pin(async move {
                        info!("定时触发任务 [{}]", tid);
                        if let Err(e) = this.trigger_task(tid).await {
                            warn!("定时任务触发失败: {}", e);
                        }
                    })
                }) {
                    Ok(job) => {
                        let _ = sched.add(job).await;
                        info!("已注册定时任务「{}」，Cron: {}", task.name, job_cron);
                    }
                    Err(e) => {
                        warn!("解析 Cron 表达式失败 ({}) 任务: {}: {}", job_cron, task.name, e);
                    }
                }
            }
        }

        sched
            .start()
            .await
            .map_err(|e| format!("启动调度器失败: {}", e))?;

        *sched_lock = Some(sched);
        Ok(())
    }
}
