use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_server_port")]
    pub port: u16,
    #[serde(default)]
    pub alist: AlistConfig,
    #[serde(default)]
    pub tasks: Vec<StrmTaskConfig>,
}

fn default_server_port() -> u16 {
    8383
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlistConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_wait_time")]
    pub wait_time: f64,
}

impl Default for AlistConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            public_url: None,
            token: String::new(),
            wait_time: 0.05,
        }
    }
}

fn default_base_url() -> String {
    "http://127.0.0.1:5244".to_string()
}

fn default_wait_time() -> f64 {
    0.05
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrmMode {
    /// 专为百度网盘等 302 挂载优化：写入 AList 的 /d/... 直链，带 sign 校验
    /// 播放时 AList 实时 302 重定向到百度网盘临时直链，永不失效
    AlistURL,
    /// 直接获取上游 raw_url 写入 strm（短效直链不推荐）
    RawURL,
}

impl Default for StrmMode {
    fn default() -> Self {
        StrmMode::AlistURL
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrmTaskConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub cron: Option<String>,
    pub source_dir: String,
    pub target_dir: String,
    #[serde(default)]
    pub mode: StrmMode,
    #[serde(default)]
    pub flatten: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub sync_delete: bool,
    #[serde(default = "default_video_extensions")]
    pub video_extensions: Vec<String>,
}

fn default_concurrency() -> usize {
    10
}

pub fn default_video_extensions() -> Vec<String> {
    vec![
        ".mp4", ".mkv", ".avi", ".ts", ".iso", ".mov", ".flv", ".wmv", ".m2ts",
        ".webm", ".rmvb", ".m4v",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 8383,
            alist: AlistConfig::default(),
            tasks: vec![StrmTaskConfig {
                id: uuid::Uuid::new_v4().to_string(),
                name: "百度网盘影视".to_string(),
                cron: Some("0 0 3 * * *".to_string()),
                source_dir: "/百度网盘/影视".to_string(),
                target_dir: "./strm/movies".to_string(),
                mode: StrmMode::AlistURL,
                flatten: false,
                overwrite: false,
                concurrency: 10,
                sync_delete: false,
                video_extensions: default_video_extensions(),
            }],
        }
    }
}

#[derive(Clone)]
pub struct ConfigStore {
    pub file_path: PathBuf,
    pub config: Arc<RwLock<AppConfig>>,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let file_path = path.into();
        let app_config = if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(content) => serde_yaml::from_str(&content).unwrap_or_else(|e| {
                    eprintln!("解析配置文件失败: {:?}，使用默认配置", e);
                    AppConfig::default()
                }),
                Err(e) => {
                    eprintln!("读取配置文件失败: {:?}，使用默认配置", e);
                    AppConfig::default()
                }
            }
        } else {
            let default_cfg = AppConfig::default();
            if let Some(parent) = file_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(content) = serde_yaml::to_string(&default_cfg) {
                let _ = std::fs::write(&file_path, content);
            }
            default_cfg
        };

        Self {
            file_path,
            config: Arc::new(RwLock::new(app_config)),
        }
    }

    pub async fn get(&self) -> AppConfig {
        self.config.read().await.clone()
    }

    pub async fn update(&self, new_config: AppConfig) -> Result<(), String> {
        let content = serde_yaml::to_string(&new_config).map_err(|e| e.to_string())?;
        if let Some(parent) = self.file_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&self.file_path, content)
            .await
            .map_err(|e| format!("保存配置文件失败: {}", e))?;

        let mut lock = self.config.write().await;
        *lock = new_config;
        Ok(())
    }
}
