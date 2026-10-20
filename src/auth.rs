use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::api::AppState;

const TOKEN_LIFETIME: Duration = Duration::from_secs(7 * 24 * 3600); // 7 天有效

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Instant>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 创建一个新令牌
    pub async fn create_token(&self) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        let mut lock = self.sessions.write().await;
        lock.insert(token.clone(), Instant::now());
        token
    }

    /// 校验令牌有效性
    pub async fn validate_token(&self, token: &str) -> bool {
        let mut lock = self.sessions.write().await;
        if let Some(created) = lock.get(token) {
            if created.elapsed() < TOKEN_LIFETIME {
                return true;
            } else {
                lock.remove(token);
            }
        }
        false
    }

    /// 注销令牌
    pub async fn revoke_token(&self, token: &str) {
        let mut lock = self.sessions.write().await;
        lock.remove(token);
    }
}

pub fn hash_password(password: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.update(salt.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn verify_password(input: &str, stored: &str, salt: &str) -> bool {
    if input == stored {
        return true;
    }
    let hashed = hash_password(input, salt);
    hashed == stored
}

fn extract_token_from_query(query: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut parts = pair.split('=');
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            if k == "token" && !v.trim().is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 身份认证中间件：保护需要鉴权的 API
pub async fn require_admin_auth(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let cfg = state.config_store.get().await;
    if !cfg.auth.enabled {
        return Ok(next.run(request).await);
    }

    // 1. 尝试从 Authorization: Bearer <token> 提取
    if let Some(auth_header) = request.headers().get(header::AUTHORIZATION) {
        if let Ok(val) = auth_header.to_str() {
            if val.starts_with("Bearer ") {
                let token = val[7..].trim();
                if state.session_manager.validate_token(token).await {
                    return Ok(next.run(request).await);
                }
            }
        }
    }

    // 2. 尝试从 Cookie 中提取 auth_token
    if let Some(cookie_header) = request.headers().get(header::COOKIE) {
        if let Ok(val) = cookie_header.to_str() {
            for part in val.split(';') {
                let pair: Vec<&str> = part.trim().split('=').collect();
                if pair.len() == 2 && pair[0] == "auth_token" {
                    if state.session_manager.validate_token(pair[1]).await {
                        return Ok(next.run(request).await);
                    }
                }
            }
        }
    }

    // 3. 尝试从 URL Query 中提取 token（支持 EventSource SSE）
    if let Some(query_str) = request.uri().query() {
        if let Some(t) = extract_token_from_query(query_str) {
            if state.session_manager.validate_token(&t).await {
                return Ok(next.run(request).await);
            }
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}
