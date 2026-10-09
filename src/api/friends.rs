//! 好友 API

//! 好友列表、添加、删除



use crate::error::{success_response, AppError, AppResult};

use crate::models::UserInfo;

use crate::state::AppState;

use crate::api::extractors::AuthUser;

use axum::{

    extract::{Path, State},

    Json,

};

use serde::Deserialize;

use serde_json::Value;

use sqlx::Row;



/// 获取好友列表

fn row_to_user_info(row: sqlx::any::AnyRow) -> UserInfo {

    UserInfo {

        id: row.try_get::<i64, _>("id").unwrap_or(0),

        username: row.try_get::<String, _>("username").unwrap_or_default(),

        nickname: row.try_get::<String, _>("nickname").unwrap_or_default(),

        avatar_url: row.try_get::<Option<String>, _>("avatar_url").unwrap_or_default(),

        status: row.try_get::<String, _>("status").unwrap_or_default(),

    }

}



/// 获取好友列表

pub async fn list_friends(

    State(state): State<AppState>,

    user: AuthUser,

) -> AppResult<Json<Vec<UserInfo>>> {

    let user_id = user.0;



    let sql = r#"

        SELECT u.id, u.username, u.nickname, u.avatar_url, u.status

        FROM users u

        JOIN friends f ON u.id = f.friend_id

        WHERE f.user_id = ? AND f.status = 'active'

        "#;



    let rows = sqlx::query(sql)

        .bind(user_id)

        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;



    let result: Vec<UserInfo> = rows

        .into_iter()

        .map(row_to_user_info)

        .collect();



    Ok(Json(result))

}



/// 添加好友

#[derive(Debug, Deserialize)]

pub struct AddFriendRequest {

    pub friend_id: i64,

}



pub async fn add_friend(

    State(state): State<AppState>,

    user: AuthUser,

    Json(req): Json<AddFriendRequest>,

) -> AppResult<Json<Value>> {

    let user_id = user.0;



    if user_id == req.friend_id {

        return Err(AppError::BadRequest("不能添加自己为好友".to_string()));

    }



    // 检查目标用户存在

    // 统一改用 fetch_all：sqlx 的 Any 驱动在 fetch_optional 命中空结果集时
    // 不归还读池连接，高并发下会迅速耗干读池（详见 auth.rs 中同名注释）。
    let exists_row = sqlx::query("SELECT id FROM users WHERE id = ?")
        .bind(req.friend_id)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;

    if exists_row.is_empty() {

        return Err(AppError::NotFound("用户不存在".to_string()));

    }



    // 检查是否已是好友或已有请求

    let existing_row = sqlx::query("SELECT id FROM friends WHERE user_id = ? AND friend_id = ?")
        .bind(user_id)
        .bind(req.friend_id)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;

    if !existing_row.is_empty() {

        return Err(AppError::BadRequest("已经是好友或请求已存在".to_string()));

    }



    // 写库：插入好友请求

    let from_uid = user_id;

    let to_uid = req.friend_id;
    state.db.spawn_write(move |pool| async move {

        let sql = "INSERT INTO friend_requests (from_user_id, to_user_id, status, created_at) VALUES (?, ?, 'pending', NOW())";

        sqlx::query(sql)

            .bind(from_uid)

            .bind(to_uid)

            .execute(&pool).await.map_err(|e| e.to_string())?;

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("发送好友请求失败: {}", e)))?;



    tracing::info!("✅ 好友请求已发送: user {} -> friend {}", user_id, req.friend_id);



    Ok(success_response(serde_json::json!({ "message": "好友请求已发送" })))

}



/// 删除好友

pub async fn remove_friend(

    State(state): State<AppState>,

    user: AuthUser,

    Path(friend_id): Path<i64>,

) -> AppResult<Json<Value>> {

    let user_id = user.0;



    // 写库：删除双向好友关系

    let uid = user_id;

    let fid = friend_id;
    state.db.spawn_write(move |pool| async move {

        let _ = sqlx::query("DELETE FROM friends WHERE user_id = ? AND friend_id = ?")

            .bind(uid).bind(fid).execute(&pool).await;

        let _ = sqlx::query("DELETE FROM friends WHERE user_id = ? AND friend_id = ?")

            .bind(fid).bind(uid).execute(&pool).await;

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("删除好友失败: {}", e)))?;



    tracing::info!("✅ 好友已删除: user {} -> friend {}", user_id, friend_id);



    Ok(success_response(serde_json::json!({ "message": "好友已删除" })))

}



fn row_to_friend_request(row: sqlx::any::AnyRow) -> serde_json::Value {

    serde_json::json!({

        "request_id": row.try_get::<i64, _>("id").unwrap_or(0),

        "user_id": row.try_get::<i64, _>("from_user_id").unwrap_or(0),

        "username": row.try_get::<String, _>("username").unwrap_or_default(),

        "nickname": row.try_get::<String, _>("nickname").unwrap_or_default(),

        "avatar_url": row.try_get::<Option<String>, _>("avatar_url").unwrap_or_default(),

        "status": row.try_get::<String, _>("status").unwrap_or_default()

    })

}



/// 获取好友请求列表

pub async fn list_friend_requests(

    State(state): State<AppState>,

    user: AuthUser,

) -> AppResult<Json<Vec<serde_json::Value>>> {

    let user_id = user.0;



    let sql = r#"

        SELECT fr.id, fr.from_user_id, u.username, u.nickname, u.avatar_url, fr.status

        FROM friend_requests fr

        JOIN users u ON fr.from_user_id = u.id

        WHERE fr.to_user_id = ? AND fr.status = 'pending'

        "#;



    let rows = sqlx::query(sql)

        .bind(user_id)

        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;



    let result: Vec<serde_json::Value> = rows

        .into_iter()

        .map(row_to_friend_request)

        .collect();



    Ok(Json(result))

}



/// 接受/拒绝好友请求

#[derive(Debug, Deserialize)]

pub struct HandleFriendRequest {

    pub request_id: i64,

    pub accept: bool,

}



pub async fn handle_friend_request(

    State(state): State<AppState>,

    user: AuthUser,

    Json(req): Json<HandleFriendRequest>,

) -> AppResult<Json<Value>> {

    let my_user_id = user.0;



    // 验证请求是发给自己的

    let mut row = sqlx::query(

        "SELECT id, from_user_id FROM friend_requests WHERE id = ? AND to_user_id = ? AND status = 'pending'"

    )

    .bind(req.request_id)

    .bind(my_user_id)

    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| AppError::Internal(format!("查询好友请求失败: {}", e)))?;

    let r = row.pop()
        .ok_or_else(|| AppError::NotFound("好友请求不存在或已处理".to_string()))?;

    let (real_request_id, from_user_id) = (

        r.try_get::<i64, _>("id").unwrap_or(0),

        r.try_get::<i64, _>("from_user_id").unwrap_or(0),

    );



    // 写库：处理好友请求

    let req_id = real_request_id;

    let new_status = if req.accept { "accepted" } else { "rejected" };

    let uid1 = my_user_id;

    let uid2 = from_user_id;
    state.db.spawn_write(move |pool| async move {

        let sql1 = "UPDATE friend_requests SET status = ? WHERE id = ?";

        sqlx::query(sql1).bind(new_status).bind(req_id).execute(&pool).await.map_err(|e| e.to_string())?;

        if req.accept {

            let sql2 = "INSERT INTO friends (user_id, friend_id, status, created_at) VALUES (?, ?, 'active', NOW())";

            sqlx::query(sql2).bind(uid1).bind(uid2).execute(&pool).await.map_err(|e| e.to_string())?;

            sqlx::query(sql2).bind(uid2).bind(uid1).execute(&pool).await.map_err(|e| e.to_string())?;

        }

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("处理好友请求失败: {}", e)))?;



    tracing::info!("✅ {}了好友请求: {} -> {}", if req.accept { "接受" } else { "拒绝" }, from_user_id, my_user_id);



    Ok(success_response(serde_json::json!({ "message": if req.accept { "已接受" } else { "已拒绝" } })))

}
