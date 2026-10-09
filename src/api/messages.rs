//! 消息 API
//!
//! 发送消息支持 REST + WS Sync 模式（参考 Chat-Api 项目）：
//! REST 发送成功后，在线接收方与发送者的其他设备会实时收到 WS 推送，
//! 与走 WebSocket 发送的消息走完全相同的推送链路。

use crate::api::extractors::AuthUser;

use crate::error::{success_response, AppError, AppResult};

use crate::models::{Message, SendMessageRequest};

use crate::ws::{message_push_json, push_to_group, push_to_user};

use sqlx::Row;

use crate::state::AppState;

use axum::{

    extract::{Query, State},

    Json,

};

use serde::Deserialize;

use serde_json::Value;



#[derive(Debug, Deserialize)]

pub struct ListQuery {

    pub limit: Option<i64>,

    pub offset: Option<i64>,

    pub friend_id: Option<i64>,

    pub group_id: Option<i64>,

}



fn row_to_message(row: sqlx::any::AnyRow) -> Message {

    let id: i64 = row.try_get("id").unwrap_or(0);

    let sender_id: i64 = row.try_get("sender_id").unwrap_or(0);

    let receiver_id: Option<i64> = row.try_get("receiver_id").ok();

    let group_id: Option<i64> = row.try_get("group_id").ok();

    let content: String = row.try_get("content").unwrap_or_default();

    let msg_type: String = row.try_get("msg_type").unwrap_or_else(|_| "text".to_string());

    let is_read: i32 = row.try_get::<i32, _>("is_read").unwrap_or(0);

    let created_at: String = row.try_get("created_at").unwrap_or_else(|_| "".to_string());

    Message { id, sender_id, receiver_id, group_id, content, msg_type, read: is_read, created_at }

}



/// 获取消息列表

pub async fn list_messages(

    user: AuthUser,

    State(state): State<AppState>,

    Query(query): Query<ListQuery>,

) -> AppResult<Json<Vec<Message>>> {

    let user_id = user.0;

    let limit = query.limit.unwrap_or(50);

    let offset = query.offset.unwrap_or(0);

    let base_select = r#"SELECT id, sender_id, receiver_id, group_id, content, msg_type, CAST(is_read AS SIGNED) as is_read, DATE_FORMAT(created_at, '%Y-%m-%d %H:%i:%s') as created_at FROM messages"#;



    let messages: Vec<Message> = if let Some(friend_id) = query.friend_id {

        let sql = format!("{} WHERE ((sender_id = ? AND receiver_id = ?) OR (sender_id = ? AND receiver_id = ?)) AND group_id IS NULL ORDER BY created_at DESC LIMIT ? OFFSET ?", base_select);

        sqlx::query(&sql).bind(user_id).bind(friend_id).bind(friend_id).bind(user_id).bind(limit).bind(offset)

            .fetch_all(&state.db.pool)

            .await

            .map_err(|e| AppError::Internal(format!("查询消息失败: {}", e)))?

            .into_iter().map(row_to_message).collect()

    } else if let Some(group_id) = query.group_id {

        let sql = format!("{} WHERE group_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?", base_select);

        sqlx::query(&sql).bind(group_id).bind(limit).bind(offset)

            .fetch_all(&state.db.pool)

            .await

            .map_err(|e| AppError::Internal(format!("查询消息失败: {}", e)))?

            .into_iter().map(row_to_message).collect()

    } else {

        let sql = format!("{} WHERE (sender_id = ? OR receiver_id = ?) AND group_id IS NULL ORDER BY created_at DESC LIMIT ? OFFSET ?", base_select);

        sqlx::query(&sql).bind(user_id).bind(user_id).bind(limit).bind(offset)

            .fetch_all(&state.db.pool)

            .await

            .map_err(|e| AppError::Internal(format!("查询消息失败: {}", e)))?

            .into_iter().map(row_to_message).collect()

    };

    Ok(Json(messages))

}



/// 发送消息（REST 入口；成功后同步 WS 推送）

pub async fn send_message(

    user: AuthUser,

    State(state): State<AppState>,

    Json(req): Json<SendMessageRequest>,

) -> AppResult<Json<Message>> {

    let user_id = user.0;

    let (receiver_id, group_id) = if req.receiver_id.is_some() {

        (req.receiver_id, None)

    } else if req.group_id.is_some() {

        (None, req.group_id)

    } else {

        return Err(AppError::BadRequest("must specify receiver_id or group_id".to_string()));

    };

    let msg_type = req.msg_type.unwrap_or_else(|| "text".to_string());

    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // 统一使用 Snowflake 生成消息 ID，支持同步/异步两种落库模式。
    let msg_id = state.snowflake.next_id();

    let message = Message {
        id: msg_id,
        sender_id: user_id,
        receiver_id,
        group_id,
        content: req.content.clone(),
        msg_type: msg_type.clone(),
        created_at: now.clone(),
        read: 0,
    };

    // 异步批量写入模式（MESSAGE_ASYNC=1）：入队后立即返回，后台每 100ms/200 条批量 flush。
    // 可绕过 innodb_flush_log_at_trx_commit=1 下单条 fsync 的瓶颈。
    if let Some(ref writer) = state.async_message_writer {
        writer.tx.send(message.clone()).await
            .map_err(|e| AppError::Internal(format!("消息入队失败: {}", e)))?;
    } else {
        // 同步写入模式：保持原有行为，直接 INSERT。
        let content = req.content.clone();
        state.db.spawn_write(move |pool| async move {
            let sql = "INSERT INTO messages (id, sender_id, receiver_id, group_id, content, msg_type, is_read, created_at) VALUES (?, ?, ?, ?, ?, ?, 0, NOW())";
            sqlx::query(sql)
                .bind(msg_id)
                .bind(user_id)
                .bind(receiver_id)
                .bind(group_id)
                .bind(&content)
                .bind(&msg_type)
                .execute(&pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        }).await.map_err(|e| AppError::Internal(format!("发送消息失败: {}", e)))?;
    }



    // REST + WS Sync：REST 发送成功后，把消息推送给在线的接收方，
    // 以及发送者的其他在线设备（走与 WS 聊天相同的推送链路/消息格式）

    let push = message_push_json(

        msg_id, user_id, receiver_id, group_id, &message.content, &message.msg_type, &now,

    );

    if let Some(rid) = receiver_id {

        push_to_user(&state.local_ws_users, rid, &push);

        push_to_user(&state.local_ws_users, user_id, &push);

    } else if let Some(gid) = group_id {

        push_to_group(&state, gid, &push).await;

    }



    tracing::info!("消息已发送: {} -> {:?}", user_id, receiver_id.or(group_id));

    Ok(Json(message))

}



/// 标记消息已读

#[derive(Debug, Deserialize)]

pub struct MarkReadRequest {

    pub message_ids: Vec<i64>,

}



pub async fn mark_messages_read(

    user: AuthUser,

    State(state): State<AppState>,

    Json(req): Json<MarkReadRequest>,

) -> AppResult<Json<Value>> {

    let _user_id = user.0;

    let ids = req.message_ids.clone();

    state.db.spawn_write(move |pool| async move {

        for mid in ids {

            let _ = sqlx::query("UPDATE messages SET is_read = 1 WHERE id = ?")

                .bind(mid).execute(&pool).await;

        }

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("标记已读失败: {}", e)))?;

    Ok(success_response(serde_json::json!({ "message": "marked as read" })))

}



/// 获取未读消息数

pub async fn get_unread_count(

    user: AuthUser,

    State(state): State<AppState>,

) -> AppResult<Json<Value>> {

    let user_id = user.0;

    let count_row = sqlx::query("SELECT COUNT(*) as count FROM messages WHERE receiver_id = ? AND is_read = 0")

        .bind(user_id)

        .fetch_one(&state.db.pool)

        .await

        .map_err(|e| AppError::Internal(format!("查询未读数失败: {}", e)))?;

    let count = count_row.try_get::<i64, _>("count").unwrap_or(0);

    Ok(success_response(serde_json::json!({ "unread_count": count })))

}
