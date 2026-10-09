//! 搜索 API
//! 搜索用户、群组

use crate::api::extractors::AuthUser;
use crate::error::{success_response, AppResult};
use crate::models::UserInfo;
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub keyword: Option<String>,
}

/// 搜索用户
pub async fn search_users(
    user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> AppResult<Json<Value>> {
    let user_id = user.0;

    let keyword = params.keyword.clone().unwrap_or_default();
    if keyword.is_empty() {
        return Ok(success_response(serde_json::json!([])));
    }

    let pattern = format!("%{}%", keyword);
    let rows5 = sqlx::query(
        r#"SELECT id, username, nickname, COALESCE(avatar_url, '') as avatar_url, status FROM users WHERE (username LIKE ? OR nickname LIKE ?) AND id != ? LIMIT 20"#
    )
    .bind(&pattern)
    .bind(&pattern)
    .bind(user_id)
    .fetch_all(&state.db.pool)
    .await?;

    let users: Vec<UserInfo> = rows5
        .into_iter()
        .map(|row| UserInfo {
            id: row.try_get::<i64, _>("id").unwrap_or(0),
            username: row.try_get::<String, _>("username").unwrap_or_default(),
            nickname: row.try_get::<String, _>("nickname").unwrap_or_default(),
            avatar_url: row.try_get::<Option<String>, _>("avatar_url").unwrap_or_default(),
            status: row.try_get::<String, _>("status").unwrap_or_default(),
        })
        .collect();

    Ok(success_response(users))
}

/// 搜索群组
pub async fn search_groups(
    user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> AppResult<Json<Value>> {
    let _user_id = user.0;

    let keyword = params.keyword.clone().unwrap_or_default();
    if keyword.is_empty() {
        return Ok(success_response(serde_json::json!([])));
    }

    let pattern = format!("%{}%", keyword);
    // groups 是 MySQL 保留字，用反引号转义
    let rows6 = sqlx::query(
        r#"SELECT g.id, CAST(g.name AS CHAR(255)) as name, COALESCE(CAST(g.description AS CHAR(500)), '') as description, COALESCE(CAST(g.avatar_url AS CHAR(500)), '') as avatar_url, g.owner_id, CAST(g.created_at AS CHAR(19)) as created_at FROM `groups` g WHERE g.name LIKE ? LIMIT 20"#
    )
    .bind(&pattern)
    .fetch_all(&state.db.pool)
    .await?;

    let groups: Vec<serde_json::Value> = rows6
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "id": row.try_get::<i64, _>("id").unwrap_or(0),
                "name": row.try_get::<String, _>("name").unwrap_or_default(),
                "description": row.try_get::<String, _>("description").unwrap_or_default(),
                "avatar_url": row.try_get::<Option<String>, _>("avatar_url").unwrap_or_default(),
                "owner_id": row.try_get::<i64, _>("owner_id").unwrap_or(0),
                "created_at": row.try_get::<String, _>("created_at").unwrap_or_default()
            })
        })
        .collect();

    Ok(success_response(groups))
}
