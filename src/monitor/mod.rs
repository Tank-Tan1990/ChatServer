//! 监控模块
//! 健康检查、性能指标、系统状态

use crate::state::AppState;
use axum::{
    extract::State,
    Json,
};
use serde::Serialize;
use std::sync::OnceLock;

/// 系统启动时间
static START_TIME: OnceLock<u64> = OnceLock::new();

fn get_start_time() -> u64 {
    *START_TIME.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// 运行时间
fn uptime() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(get_start_time())
}

/// 健康检查响应
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
    pub timestamp: String,
}

/// 详细系统状态
#[derive(Serialize)]
pub struct SystemStatus {
    pub health: HealthResponse,
    pub database: DatabaseStatus,
    pub websocket: WebSocketStatus,
}

/// 数据库状态
#[derive(Serialize)]
pub struct DatabaseStatus {
    pub connected: bool,
    pub pool_size: u32,
    /// 读池当前实际连接数（含在用与空闲）
    pub read_pool_size: u32,
    /// 读池空闲连接数
    pub read_pool_idle: usize,
    /// 读池「已被借出且未归还」的连接数 —— 持续增长即为连接泄漏
    pub read_pool_in_use: u32,
    /// 写池当前实际连接数
    pub write_pool_size: u32,
    /// 写池空闲连接数
    pub write_pool_idle: usize,
    /// 写池「已被借出且未归还」的连接数
    pub write_pool_in_use: u32,
    /// 写信号量当前可用 permit 数
    pub write_sem_available: usize,
}

/// WebSocket 状态
#[derive(Serialize)]
pub struct WebSocketStatus {
    pub connected_users: usize,
}

/// 健康检查
pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: uptime(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    })
}

/// 详细系统状态
pub async fn system_status(
    State(state): State<AppState>,
) -> Json<SystemStatus> {
    let online_count = state.local_ws_users.read().len();
    let pool_size = state.db.read_pool_max;

    // 真实池指标：size() 是池内连接总数，num_idle() 是空闲数，
    // 两者之差即「已借出未归还」的连接 —— 用于定位连接泄漏。
    let read_size = state.db.read_pool.size();
    let read_idle = state.db.read_pool.num_idle();
    let write_size = state.db.write_pool.size();
    let write_idle = state.db.write_pool.num_idle();

    Json(SystemStatus {
        health: HealthResponse {
            status: "ok".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: uptime(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
        database: DatabaseStatus {
            connected: true,
            pool_size,
            read_pool_size: read_size,
            read_pool_idle: read_idle,
            read_pool_in_use: read_size.saturating_sub(read_idle as u32),
            write_pool_size: write_size,
            write_pool_idle: write_idle,
            write_pool_in_use: write_size.saturating_sub(write_idle as u32),
            write_sem_available: state.db.write_sem.available_permits(),
        },
        websocket: WebSocketStatus {
            connected_users: online_count,
        },
    })
}