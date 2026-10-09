//! WebSocket 管理 API

use crate::error::AppResult;
use crate::state::AppState;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct WsStats {
    pub connection_count: usize,
    pub online_users: Vec<i64>,
    pub max_connections: i64,
    pub utilization_percent: f64,
}

/// 获取 WebSocket 连接统计
pub async fn ws_stats(
    State(state): State<AppState>,
) -> AppResult<Json<WsStats>> {
    let ws_users = state.local_ws_users.read();
    // 多端在线：连接数 = 所有用户的设备连接之和，在线用户数 = uid 数
    let count: usize = ws_users.values().map(|conns| conns.len()).sum();
    let users: Vec<i64> = ws_users.keys().copied().collect();
    let max = 15000;
    let stats = WsStats {
        connection_count: count,
        online_users: users,
        max_connections: max,
        utilization_percent: if max > 0 { (count as f64 / max as f64) * 100.0 } else { 0.0 },
    };
    Ok(Json(stats))
}
