use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info};

#[derive(Clone)]
pub struct AlistClient {
    client: Client,
    pub base_url: String,
    pub public_url: Option<String>,
    #[allow(dead_code)]
    pub token: String,
    pub wait_time: Duration,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AlistResponse<T> {
    pub code: i32,
    pub message: String,
    pub data: Option<T>,
}

/// AList 的 `role` 字段兼容层。
///
/// 旧版本返回单个整数（如 `"role": 2`），新版 beta（支持多角色）返回数组
/// （如 `"role": [2]`）。两种格式都接受，避免升级 AList 后 `/api/me`
/// 反序列化失败（表现为 `error decoding response body`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AlistRole {
    Single(i32),
    Multiple(Vec<i32>),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AlistUserInfo {
    pub username: Option<String>,
    #[serde(default = "default_base_path")]
    pub base_path: String,
    pub role: Option<AlistRole>,
}

fn default_base_path() -> String {
    "/".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AlistFsItem {
    pub name: String,
    pub size: i64,
    pub is_dir: bool,
    #[serde(default)]
    pub sign: String,
    #[serde(default)]
    pub modified: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AlistFsListData {
    #[serde(default)]
    pub content: Option<Vec<AlistFsItem>>,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AlistFsGetItem {
    pub name: String,
    pub size: i64,
    pub is_dir: bool,
    #[serde(default)]
    pub sign: String,
    #[serde(default)]
    pub raw_url: Option<String>,
}

impl AlistClient {
    pub fn new(
        base_url: String,
        public_url: Option<String>,
        token: String,
        wait_time_secs: f64,
    ) -> Result<Self, String> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("alist2strm/1.0"),
        );

        let token_trimmed = token.trim();
        if !token_trimmed.is_empty() {
            let mut val = header::HeaderValue::from_str(token_trimmed)
                .map_err(|e| format!("无效的 Token 格式: {}", e))?;
            val.set_sensitive(true);
            headers.insert(header::AUTHORIZATION, val);
        }

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;

        let clean_base = base_url.trim_end_matches('/').to_string();
        let clean_public = public_url
            .map(|u| u.trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty());

        Ok(Self {
            client,
            base_url: clean_base,
            public_url: clean_public,
            token: token_trimmed.to_string(),
            wait_time: Duration::from_secs_f64(wait_time_secs.max(0.0)),
        })
    }

    /// 获取供外部播放的 Base URL（若配置了 public_url 则使用，否则回退到 base_url）
    pub fn effective_public_url(&self) -> &str {
        if let Some(ref pub_url) = self.public_url {
            pub_url.as_str()
        } else {
            self.base_url.as_str()
        }
    }

    /// 测试连接并获取当前用户及 base_path
    pub async fn test_connection(&self) -> Result<AlistUserInfo, String> {
        let url = format!("{}/api/me", self.base_url);
        debug!("测试 AList 连接: {}", url);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("无法连接 AList 服务器 ({}): {}", url, e))?;

        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err("认证失败：Token 无效或权限不足".to_string());
        }

        let body: AlistResponse<AlistUserInfo> = {
            let body_text = resp
                .text()
                .await
                .map_err(|e| format!("读取 AList 用户信息响应失败: {}", e))?;

            serde_json::from_str(&body_text).map_err(|e| {
                // reqwest 的 `.json()` 只会把错误显示成 "error decoding response body"，
                // 这里保留 Serde 的具体原因和响应片段，方便排查字段变更。
                let preview: String = body_text.chars().take(200).collect();
                format!("解析 AList 用户信息响应失败: {}；响应内容: {}", e, preview)
            })?
        };

        if body.code != 200 {
            return Err(format!("AList 返回错误 ({}): {}", body.code, body.message));
        }

        let mut info = body.data.unwrap_or(AlistUserInfo {
            username: Some("Guest".into()),
            base_path: "/".into(),
            role: None,
        });

        if info.base_path.is_empty() {
            info.base_path = "/".to_string();
        }

        info!(
            "AList 连接测试成功！用户: {:?}, BasePath: {}",
            info.username, info.base_path
        );
        Ok(info)
    }

    /// 列出指定目录下的文件与文件夹
    pub async fn list_dir(&self, dir_path: &str) -> Result<Vec<AlistFsItem>, String> {
        if self.wait_time > Duration::ZERO {
            tokio::time::sleep(self.wait_time).await;
        }

        let url = format!("{}/api/fs/list", self.base_url);
        let payload = serde_json::json!({
            "path": dir_path,
            "password": "",
            "page": 1,
            "per_page": 0,
            "refresh": false
        });

        debug!("正在扫描 AList 目录: {}", dir_path);

        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("请求目录列表失败 ({}): {}", dir_path, e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "请求目录 {} 返回 HTTP 错误: {}",
                dir_path,
                resp.status()
            ));
        }

        let body: AlistResponse<AlistFsListData> = resp
            .json()
            .await
            .map_err(|e| format!("解析目录列表失败 ({}): {}", dir_path, e))?;

        if body.code != 200 {
            return Err(format!("AList 错误 ({}): {}", body.code, body.message));
        }

        let items = body
            .data
            .and_then(|d| d.content)
            .unwrap_or_default();

        Ok(items)
    }

    /// 获取单个文件详情（RawURL 模式备用）
    pub async fn get_fs(&self, file_path: &str) -> Result<AlistFsGetItem, String> {
        if self.wait_time > Duration::ZERO {
            tokio::time::sleep(self.wait_time).await;
        }

        let url = format!("{}/api/fs/get", self.base_url);
        let payload = serde_json::json!({
            "path": file_path,
            "password": ""
        });

        let resp = self
            .client
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("获取文件详情失败 ({}): {}", file_path, e))?;

        let body: AlistResponse<AlistFsGetItem> = resp
            .json()
            .await
            .map_err(|e| format!("解析文件详情失败: {}", e))?;

        if body.code != 200 {
            return Err(format!("AList 错误 ({}): {}", body.code, body.message));
        }

        body.data
            .ok_or_else(|| format!("未返回文件数据: {}", file_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_user_info(raw: &str) -> AlistUserInfo {
        let resp: AlistResponse<AlistUserInfo> =
            serde_json::from_str(raw).expect("用户信息响应应当解析成功");
        assert_eq!(resp.code, 200);
        resp.data.expect("响应应当包含 data")
    }

    #[test]
    fn parses_legacy_single_role() {
        let info = parse_user_info(
            r#"{"code":200,"message":"success","data":{"id":1,"username":"admin","base_path":"/","role":2}}"#,
        );

        assert_eq!(info.username.as_deref(), Some("admin"));
        assert_eq!(info.base_path, "/");
        assert_eq!(info.role, Some(AlistRole::Single(2)));
    }

    #[test]
    fn parses_beta_role_array() {
        // 抓取自 AList beta（2026-10-13 构建）的真实响应
        let info = parse_user_info(
            r#"{"code":200,"message":"success","data":{"id":2,"username":"admin","password":"","base_path":"/","role":[2],"disabled":false,"permission":65535,"sso_id":"","otp":false,"role_names":null,"permissions":null}}"#,
        );

        assert_eq!(info.username.as_deref(), Some("admin"));
        assert_eq!(info.base_path, "/");
        assert_eq!(info.role, Some(AlistRole::Multiple(vec![2])));
    }

    #[test]
    fn parses_multiple_roles() {
        let info = parse_user_info(
            r#"{"code":200,"message":"success","data":{"username":"admin","base_path":"/","role":[1,2]}}"#,
        );

        assert_eq!(info.role, Some(AlistRole::Multiple(vec![1, 2])));
    }

    #[test]
    fn parses_missing_or_null_role() {
        let missing = parse_user_info(
            r#"{"code":200,"message":"success","data":{"username":"guest","base_path":"/"}}"#,
        );
        assert_eq!(missing.role, None);

        let null = parse_user_info(
            r#"{"code":200,"message":"success","data":{"username":"guest","base_path":"/","role":null}}"#,
        );
        assert_eq!(null.role, None);
    }
}
