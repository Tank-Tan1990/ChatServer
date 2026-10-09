//! WebSocket 模块
//! 实时消息通信 + 心跳机制 + 多端在线支持
//!
//! 连接管理设计（参考 OpenIM/WuKongIM 的网关层模式）：
//! - 每个连接有全局唯一 conn_id，同一 uid 可多设备同时在线；
//! - 每个连接持有一条专属 broadcast 通道，断开时只移除自己的通道，
//!   不会顶掉同用户的其他设备；
//! - 推送统一走 push_to_user / push_to_group，REST 发消息与 WS 发消息
//!   共享同一条推送链路（REST + WS Sync 模式）。

use crate::error::AppError;
use crate::snowflake::Snowflake;
use crate::state::{AppState, UserConnections};
use crate::api::auth::{decode_token, get_jwt_secret};
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use serde::Deserialize;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// 连接 ID 分配器（全局自增，用于区分同一用户的多个设备）
static CONN_ID: AtomicU64 = AtomicU64::new(1);

fn next_conn_id() -> u64 {
    CONN_ID.fetch_add(1, Ordering::Relaxed)
}

/// 把消息推给指定用户的所有在线设备（单聊推送的统一入口）
pub fn push_to_user(
    users: &Arc<RwLock<HashMap<i64, UserConnections>>>,
    uid: i64,
    msg: &str,
) {
    let conns = users.read().get(&uid).cloned();
    if let Some(conns) = conns {
        for (_conn_id, tx) in conns.iter() {
            if let Err(e) = tx.send(msg.to_string()) {
                tracing::debug!("[push_to_user] uid={} 推送失败: {}", uid, e);
            }
        }
    }
}

/// 群消息广播给所有在线成员（含发送者的全部设备）。
/// 注意 group_members 表没有 id 列（联合主键 group_id+user_id），只能取 user_id。
pub async fn push_to_group(state: &AppState, group_id: i64, msg: &str) {
    let rows = sqlx::query("SELECT user_id FROM group_members WHERE group_id = ?")
        .bind(group_id)
        .fetch_all(&state.db.pool)
        .await;
    match rows {
        Ok(rows) => {
            for row in rows {
                if let Ok(uid) = row.try_get::<i64, _>("user_id") {
                    push_to_user(&state.local_ws_users, uid, msg);
                }
            }
        }
        Err(e) => {
            tracing::error!("查询群成员失败，群消息未广播: {}", e);
        }
    }
}

/// 构造统一的 WS 消息推送载荷（REST 与 WS 发送路径共用同一格式）
pub fn message_push_json(
    msg_id: i64,
    sender_id: i64,
    receiver_id: Option<i64>,
    group_id: Option<i64>,
    content: &str,
    msg_type: &str,
    timestamp: &str,
) -> String {
    serde_json::json!({
        "type": "message",
        "id": msg_id,
        "sender_id": sender_id,
        "receiver_id": receiver_id,
        "group_id": group_id,
        "content": content,
        "msg_type": msg_type,
        "timestamp": timestamp
    })
    .to_string()
}

/// WebSocket 连接处理
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// 处理 WebSocket 连接
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let conn_id = next_conn_id();

    // 协议层回执（Ping -> Pong）走这个通道，由 send_task 统一写出，
    // 因为 socket 的写半边（sender）只能在单一任务里使用。
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();

    let user_id: Arc<RwLock<Option<i64>>> = Arc::new(RwLock::new(None));
    let tx = state.local_ws_users.clone();
    let db = state.db.clone();
    let redis = state.redis.clone();
    let snowflake = state.snowflake.clone();

    let recv_task = {
        let user_id = user_id.clone();
        let tx = tx.clone();
        let db = db.clone();
        let redis = redis.clone();
        let snowflake = snowflake.clone();
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = receiver.next().await {
                match msg {
                    Ok(WsMessage::Text(text)) => {
                        if let Err(e) = handle_text_message(
                            &text,
                            conn_id,
                            user_id.clone(),
                            tx.clone(),
                            db.clone(),
                            redis.clone(),
                            snowflake.clone(),
                        ).await {
                            tracing::error!("处理 WebSocket 消息失败: {}", e);
                        }
                    }
                    Ok(WsMessage::Ping(data)) => {
                        let _ = out_tx.send(WsMessage::Pong(data));
                    }
                    Ok(WsMessage::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        })
    };

    // 发送任务：订阅本连接的广播队列，把服务端推送（auth_success / 新消息 / pong）
    // 真正写回客户端。此前这里是空转 sleep(30s)，导致：
    //   1) 客户端收不到任何推送（实时消息形同虚设）
    //   2) 连接 30 秒后被 select! 判定结束而强制断开
    let send_task = {
        let user_id = user_id.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            // 等客户端完成 auth：握手后 30 秒内未认证则断开（避免挂着的匿名连接占资源）
            let mut waited_ms: u32 = 0;
            let uid: Option<i64> = loop {
                if let Some(u) = *user_id.read() {
                    break Some(u);
                }
                if waited_ms >= 30_000 {
                    break None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                waited_ms += 100;
            };

            // 只订阅本连接（conn_id）的通道；同 uid 的其他设备互不影响
            let mut broadcast_rx = match uid {
                Some(u) => tx
                    .read()
                    .get(&u)
                    .and_then(|conns| conns.get(&conn_id))
                    .map(|s| s.subscribe()),
                None => None,
            };
            if broadcast_rx.is_none() {
                tracing::warn!("WebSocket 连接未在 30 秒内完成认证，已断开");
                return;
            }
            let uid = uid.unwrap();

            // 订阅成功后立即回执认证结果（先订阅再回执，客户端不会漏掉第一条推送）
            if sender.send(WsMessage::Text(serde_json::json!({
                "type": "auth_success",
                "user_id": uid
            }).to_string())).await.is_err() {
                return;
            }

            loop {
                let broadcast_fut = async {
                    match broadcast_rx.as_mut() {
                        Some(r) => r.recv().await.ok(),
                        None => std::future::pending::<Option<String>>().await,
                    }
                };
                tokio::select! {
                    Some(m) = out_rx.recv() => {
                        if sender.send(m).await.is_err() {
                            break;
                        }
                    }
                    Some(text) = broadcast_fut => {
                        if sender.send(WsMessage::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    else => break,
                }
            }
        })
    };

    tokio::select! { _ = recv_task => {}, _ = send_task => {} };

    // 用户断开时清理：只移除本连接，同用户其他设备不受影响
    let uid = user_id.read().clone();
    if let Some(uid) = uid {
        let still_has_devices = {
            let mut map = tx.write();
            match map.get_mut(&uid) {
                Some(conns) => {
                    conns.remove(&conn_id);
                    if conns.is_empty() {
                        map.remove(&uid);
                        false
                    } else {
                        true
                    }
                }
                None => false,
            }
        };
        // 所有设备都下线了，才更新离线状态
        if !still_has_devices {
            let uid2 = uid;
            let _ = db.spawn_write(move |pool| async move {
                sqlx::query("UPDATE users SET status = 'offline' WHERE id = ?")
                    .bind(uid2)
                    .execute(&pool)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            }).await;
            let _ = redis.user_offline(uid);
            tracing::info!("用户 {} 已离线（全部设备）", uid);
        }
    }
}

/// WebSocket 消息类型
#[derive(Deserialize)]
#[serde(tag = "type")]
enum WsMsg {
    #[serde(rename = "auth")]
    Auth { token: String },
    #[serde(rename = "chat")]
    Chat {
        receiver_id: Option<i64>,
        group_id: Option<i64>,
        content: String,
    },
    #[serde(rename = "ping")]
    Ping,
}

/// 处理文本消息
async fn handle_text_message(
    text: &str,
    conn_id: u64,
    user_id: Arc<RwLock<Option<i64>>>,
    tx: Arc<RwLock<HashMap<i64, UserConnections>>>,
    db: crate::db::DbPool,
    redis: crate::redis::RedisPool,
    snowflake: Arc<Snowflake>,
) -> Result<(), AppError> {
    let msg: WsMsg = serde_json::from_str(text)
        .map_err(|e| AppError::WebSocket(format!("消息格式错误: {}", e)))?;

    match msg {
        WsMsg::Auth { token } => {
            let claims = decode_token(&token, get_jwt_secret())?;
            let uid: i64 = claims.sub.parse()
                .map_err(|_| AppError::Auth("无效的 Token".to_string()))?;

            // 先注册广播通道、再置 user_id：
            // send_task 以 user_id 变为 Some 为信号去订阅广播通道，
            // 若先置位后插入，存在订阅瞬间拿不到通道而被误断连的竞态。
            // entry().or_default() 保证同 uid 多设备各自持有独立通道。
            let (broadcast_tx, _) = broadcast::channel(100);
            tx.write().entry(uid).or_default().insert(conn_id, broadcast_tx);
            *user_id.write() = Some(uid);

            // 写库：更新在线状态
            let uid2 = uid;
            let _ = db.spawn_write(move |pool| async move {
                sqlx::query("UPDATE users SET status = 'online' WHERE id = ?")
                    .bind(uid2)
                    .execute(&pool)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            }).await;

            let _ = redis.user_online(uid);

            // auth_success 由 send_task 在成功订阅广播通道后直接发回客户端，
            // 保证时序：客户端收到的第一条推送一定是 auth_success。

            tracing::info!("用户 {} WebSocket 已连接（conn {}）", uid, conn_id);
        }

        WsMsg::Chat { receiver_id, group_id, content } => {
            let uid = user_id.read().clone()
                .ok_or_else(|| AppError::WebSocket("未认证的连接".to_string()))?;

            // 写库：使用 Snowflake 预生成消息 ID，兼容 messages 表 id 非自增的现状
            let sender_id = uid;
            let receiver = receiver_id;
            let grp = group_id;
            let msg_content = content.clone();
            let msg_id = snowflake.next_id();

            db.spawn_write(move |pool| async move {
                let sql = "INSERT INTO messages (id, sender_id, receiver_id, group_id, content, msg_type, is_read, created_at) VALUES (?, ?, ?, ?, ?, 'text', 0, NOW())";
                sqlx::query(sql)
                    .bind(msg_id)
                    .bind(sender_id)
                    .bind(receiver)
                    .bind(grp)
                    .bind(&msg_content)
                    .execute(&pool)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            })
            .await
            .map_err(|e| AppError::Internal(format!("消息写入失败: {}", e)))?;

            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let msg_json = message_push_json(
                msg_id, uid, receiver_id, group_id, &content, "text", &now,
            );

            if let Some(rid) = receiver_id {
                // 单聊：推给接收者全部在线设备 + 发送者其他设备（多端同步）
                push_to_user(&tx, rid, &msg_json);
                push_to_user(&tx, uid, &msg_json);
            }

            if let Some(gid) = group_id {
                // 群聊：广播给全部在线成员（含发送者的所有设备）
                push_to_group_state(&db, &tx, gid, &msg_json).await;
            }

            tracing::info!("消息 {} -> {:?}", uid, receiver_id.or(group_id));
        }

        WsMsg::Ping => {
            let uid = user_id.read().clone();
            if let Some(uid) = uid {
                let _ = redis.update_heartbeat(uid);
                push_to_user(&tx, uid, r#"{"type":"pong"}"#);
            }
        }
    }
    Ok(())
}

/// 群消息广播（handle_text_message 内部用，避免借用整个 AppState）
async fn push_to_group_state(
    db: &crate::db::DbPool,
    tx: &Arc<RwLock<HashMap<i64, UserConnections>>>,
    group_id: i64,
    msg: &str,
) {
    let rows = sqlx::query("SELECT user_id FROM group_members WHERE group_id = ?")
        .bind(group_id)
        .fetch_all(&db.pool)
        .await;
    match rows {
        Ok(rows) => {
            for row in rows {
                if let Ok(uid) = row.try_get::<i64, _>("user_id") {
                    push_to_user(tx, uid, msg);
                }
            }
        }
        Err(e) => {
            tracing::error!("查询群成员失败，群消息未广播: {}", e);
        }
    }
}
