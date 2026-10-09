//! 中间件模块
//! 请求日志、限流、CORS、请求超时等中间件

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::Instant;

use crate::state::AppState;

/// 请求日志中间件
pub async fn logging_middleware(
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let start = Instant::now();

    tracing::info!("📥 {} {}", method, uri);

    let response = next.run(request).await;

    let duration = start.elapsed();
    let status = response.status();

    tracing::info!("📤 {} {} - {:?} ({:?})", method, uri, status, duration);

    response
}

/// 请求超时中间件
pub async fn timeout_middleware(
    request: Request<Body>,
    next: Next,
) -> Response {
    // 在生产环境中可以使用 `tokio::time::timeout`
    next.run(request).await
}

/// Rate Limit 响应头
#[derive(Clone)]
pub struct RateLimitHeaders {
    pub remaining: u32,
    pub reset_at: u64,
}

/// 限流中间件（基于 IP）
/// 注意：生产环境应使用 Redis 分布式限流，此为轻量级内存版
///
/// rpm 来源（env 优先、config.yaml 兜底，在 main.rs 启动时解析好放进 AppState）：
///   1. 环境变量 RATE_LIMIT_RPM（压测时用，优先级最高）
///   2. config.yaml 的 rate_limit.requests_per_minute
///   3. 默认 100000（压测友好值）
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    // 启动时已解析好的限流值（不再每个请求读一次环境变量）
    let rpm: u32 = state.rate_limit_rpm;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // 获取客户端 IP
    let ip = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("").trim().to_string())
        .or_else(|| {
            request
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    // 静态计数器（简单实现，重启后重置）
    static COUNTERS: once_cell::sync::Lazy<Arc<RwLock<HashMap<String, (u32, u64)>>>> =
        once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

    {
        let mut counters = COUNTERS.write();
        let entry = counters.entry(ip).or_insert((0, now));

        // 窗口重置（60秒）
        if now - entry.1 >= 60 {
            entry.0 = 0;
            entry.1 = now;
        }

        entry.0 += 1;

        if entry.0 > rpm {
            let reset_at = (entry.1 + 60) * 1000;
            let response = (
                StatusCode::TOO_MANY_REQUESTS,
                [
                    ("X-RateLimit-Limit", rpm.to_string()),
                    ("X-RateLimit-Remaining", "0".to_string()),
                    ("X-RateLimit-Reset", reset_at.to_string()),
                    ("Retry-After", (entry.1 + 60 - now).to_string()),
                ],
                Json(json!({
                    "error": "too_many_requests",
                    "message": format!("请求过于频繁，请在 {} 秒后重试", entry.1 + 60 - now),
                })),
            );
            return response.into_response();
        }
    }

    // 正常放行，附加限流头
    let response = next.run(request).await;
    response
}