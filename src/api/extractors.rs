//! 认证提取器（参考 realworld-axum-sqlx 的 AuthUser / MaybeAuthUser 模式）
//!
//! 用法：handler 参数里直接写 `user: AuthUser`，即可拿到已认证的 user_id，
//! 取代过去每个 handler 手动解析 `?token=` 再调 extract_user_id 的散装写法。
//!
//! token 来源优先级：
//!   1. `Authorization: Bearer <token>` 请求头（标准方式，新增）
//!   2. `?token=<token>` 查询参数（向后兼容，旧客户端/测试脚本仍在用）

use crate::api::auth::extract_user_id;
use crate::error::AppError;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// 已认证用户（必须携带有效 token，否则 401）
pub struct AuthUser(pub i64);

/// 从请求中提取 token：优先 Authorization 头，其次 query 参数
fn extract_token_from_parts(parts: &Parts) -> Option<String> {
    // 1) Authorization: Bearer <token>
    if let Some(v) = parts.headers.get(axum::http::header::AUTHORIZATION) {
        if let Ok(s) = v.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    // 2) ?token=<token>（JWT base64url 字符集不含 & = %，无需解码）
    parts.uri.query().and_then(|q| {
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            if k == "token" && !v.is_empty() {
                Some(v.to_string())
            } else {
                None
            }
        })
    })
}

#[axum::async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_token_from_parts(parts)
            .ok_or_else(|| AppError::Auth("缺少认证 token（支持 Authorization: Bearer 或 ?token=）".to_string()))?;
        let user_id = extract_user_id(&token)?;
        Ok(AuthUser(user_id))
    }
}
