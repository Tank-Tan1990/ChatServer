//! 错误类型定义
//! ChatServer 统一错误处理

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// 自定义错误类型
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("数据库错误: {0}")]
    Database(#[from] sqlx::Error),

    #[error("认证错误: {0}")]
    Auth(String),

    #[error("未找到资源: {0}")]
    NotFound(String),

    #[error("参数错误: {0}")]
    BadRequest(String),

    #[error("服务器内部错误: {0}")]
    Internal(String),

    #[error("WebSocket 错误: {0}")]
    WebSocket(String),

    #[error("限流错误: {0}")]
    RateLimit(String),

    #[error("服务器繁忙: {0}")]
    TooManyRequests(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, error_message) = match self {
            AppError::Database(e) => (StatusCode::INTERNAL_SERVER_ERROR, 50001, e.to_string()),
            AppError::Auth(msg) => (StatusCode::UNAUTHORIZED, 40101, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, 40401, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, 40001, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, 50002, msg),
            AppError::WebSocket(msg) => (StatusCode::INTERNAL_SERVER_ERROR, 50003, msg),
            AppError::RateLimit(msg) => (StatusCode::TOO_MANY_REQUESTS, 42901, msg),
            AppError::TooManyRequests(msg) => (StatusCode::TOO_MANY_REQUESTS, 42902, msg),
        };

        let body = Json(json!({
            "code": code,
            "error": error_message,
            "success": false,
        }));

        (status, body).into_response()
    }
}

/// API 响应封装
pub type AppResult<T> = Result<T, AppError>;

/// 统一成功响应
pub fn success_response<T: serde::Serialize>(data: T) -> Json<serde_json::Value> {
    Json(json!({
        "code": 0,
        "data": data,
        "success": true,
    }))
}