//! 熔断器状态监控 API

use axum::Json;
use serde_json::{json, Value};
use crate::error::AppResult;

/// 获取熔断器状态（占位）
pub async fn circuit_status() -> AppResult<Json<Value>> {
    Ok(Json(json!({
        "state": "closed",
        "total_requests": 0,
        "total_failures": 0,
        "error_rate": "0.00%",
        "message": "circuit breaker not integrated yet"
    })))
}
