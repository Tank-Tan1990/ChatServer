//! 群组 API



use crate::api::extractors::AuthUser;

use crate::error::{AppError, AppResult};

use crate::models::{CreateGroupRequest, Group};

use crate::state::AppState;

use sqlx::Row;

use axum::{

    extract::{Path, State},

    Json,

};

use serde::Deserialize;

use serde_json::Value;



/// 获取群组列表

pub async fn list_groups(

    State(state): State<AppState>,

    user: AuthUser,

) -> AppResult<Json<Vec<Group>>> {

    let user_id = user.0;



    let groups: Vec<Group> = sqlx::query_as(

        r#"SELECT g.id, g.name, COALESCE(g.avatar_url, '') as avatar_url, g.owner_id,

           DATE_FORMAT(g.created_at, '%Y-%m-%d %H:%i:%s') as created_at FROM `groups` g

           JOIN group_members gm ON g.id = gm.group_id

           WHERE gm.user_id = ?

           ORDER BY g.created_at DESC"#,

    )

    .bind(user_id)

    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;



    Ok(Json(groups))

}



/// 创建群组

pub async fn create_group(

    State(state): State<AppState>,

    user: AuthUser,

    Json(req): Json<CreateGroupRequest>,

) -> AppResult<Json<Group>> {

    let user_id = user.0;



    if req.name.trim().is_empty() {

        return Err(AppError::BadRequest("群组名称不能为空".to_string()));

    }



    // 写库：创建群组并加入

    let group_name = req.name.clone();

    let owner = user_id;
    let group_id: i64 = state.db.spawn_write(move |pool| async move {

        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

        let sql1 = "INSERT INTO `groups` (name, owner_id, created_at) VALUES (?, ?, NOW())";

        sqlx::query(sql1).bind(&group_name).bind(owner).execute(&mut *tx).await.map_err(|e| e.to_string())?;

        let sql2 = "INSERT INTO group_members (group_id, user_id, role, joined_at) VALUES (LAST_INSERT_ID(), ?, 'owner', NOW())";

        sqlx::query(sql2).bind(owner).execute(&mut *tx).await.map_err(|e| e.to_string())?;

        let row = sqlx::query("SELECT LAST_INSERT_ID() as id")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        let id: i64 = row.try_get::<i64, _>("id").unwrap_or(0);

        tx.commit().await.map_err(|e| e.to_string())?;

        Ok(id)

    }).await.map_err(|e| AppError::Internal(format!("创建群组失败: {}", e)))?;

    if group_id == 0 {

        return Err(AppError::Internal("创建群组失败：未获取到群组ID".to_string()));

    }



    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let group = Group {

        id: group_id,

        name: req.name,

        owner_id: user_id,

        avatar_url: req.avatar_url,

        created_at: now,

    };



    tracing::info!("群组创建成功: {} by {}", group.name, user_id);

    Ok(Json(group))

}



/// 获取群成员列表

pub async fn list_group_members(

    State(state): State<AppState>,

    Path(group_id): Path<i64>,

    user: AuthUser,

) -> AppResult<Json<Vec<Value>>> {

    let _user_id = user.0;



    let members_rows = sqlx::query(

        r#"SELECT u.id, u.username, u.nickname, gm.role FROM group_members gm JOIN users u ON gm.user_id = u.id WHERE gm.group_id = ?"#

    )

    .bind(group_id)

    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;



    let result: Vec<Value> = members_rows

        .into_iter()

        .map(|row| {

            serde_json::json!({

                "id": row.try_get::<i64, _>("id").unwrap_or(0),

                "username": row.try_get::<String, _>("username").unwrap_or_default(),

                "nickname": row.try_get::<String, _>("nickname").unwrap_or_default(),

                "role": row.try_get::<String, _>("role").unwrap_or_default()

            })

        })

        .collect();



    Ok(Json(result))

}



/// 添加群成员

#[derive(Debug, Deserialize)]

pub struct AddMemberRequest {

    pub user_id: i64,

}



pub async fn add_group_member(

    State(state): State<AppState>,

    Path(group_id): Path<i64>,

    user: AuthUser,

    Json(req): Json<AddMemberRequest>,

) -> AppResult<Json<Value>> {

    let operator_id = user.0;



    // 检查操作者是否是群主或管理员

    let mut role_row = sqlx::query(

        "SELECT role FROM group_members WHERE group_id = ? AND user_id = ?",

    )

    .bind(group_id)

    .bind(operator_id)

    // 改用 fetch_all：避免 fetch_optional 空结果集不还连接的缺陷
    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;

    let role = role_row.pop().map(|r| r.try_get::<String, _>("role").unwrap_or_default());

    match role.as_deref() {

        Some("owner") | Some("admin") => {}

        _ => return Err(AppError::Auth("无权限添加成员".to_string())),

    }



    // 写库：添加群成员

    let gid = group_id;

    let uid = req.user_id;
    state.db.spawn_write(move |pool| async move {

        let sql = "INSERT INTO group_members (group_id, user_id, role, joined_at) VALUES (?, ?, 'member', NOW())";

        sqlx::query(sql).bind(gid).bind(uid).execute(&pool).await.map_err(|e| e.to_string())?;

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("添加成员失败: {}", e)))?;

    Ok(Json(serde_json::json!({ "message": "成员已添加" })))

}



/// 移除群成员

pub async fn remove_group_member(

    State(state): State<AppState>,

    Path((group_id, member_id)): Path<(i64, i64)>,

    user: AuthUser,

) -> AppResult<Json<Value>> {

    let operator_id = user.0;



    let mut role_row2 = sqlx::query(

        "SELECT role FROM group_members WHERE group_id = ? AND user_id = ?",

    )

    .bind(group_id)

    .bind(operator_id)

    // 改用 fetch_all：避免 fetch_optional 空结果集不还连接的缺陷
    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| AppError::Internal(format!("查询失败: {}", e)))?;

    let role2 = role_row2.pop().map(|r| r.try_get::<String, _>("role").unwrap_or_default());

    match role2.as_deref() {

        Some("owner") | Some("admin") => {}

        _ => return Err(AppError::Auth("无权限移除成员".to_string())),

    }



    // 写库：移除群成员

    let gid = group_id;

    let mid = member_id;
    state.db.spawn_write(move |pool| async move {

        let sql = "DELETE FROM group_members WHERE group_id = ? AND user_id = ?";

        sqlx::query(sql).bind(gid).bind(mid).execute(&pool).await.map_err(|e| e.to_string())?;

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("移除成员失败: {}", e)))?;

    Ok(Json(serde_json::json!({ "message": "成员已移除" })))

}



/// 退出群组

pub async fn leave_group(

    State(state): State<AppState>,

    Path(group_id): Path<i64>,

    user: AuthUser,

) -> AppResult<Json<Value>> {

    let user_id = user.0;



    // 写库：退出群组

    let gid = group_id;

    let uid = user_id;
    state.db.spawn_write(move |pool| async move {

        let sql = "DELETE FROM group_members WHERE group_id = ? AND user_id = ?";

        sqlx::query(sql).bind(gid).bind(uid).execute(&pool).await.map_err(|e| e.to_string())?;

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("退出群组失败: {}", e)))?;



    Ok(Json(serde_json::json!({ "message": "已退出群组" })))

}



/// 获取群详情（含成员数）

pub async fn get_group_info(

    State(state): State<AppState>,

    Path(group_id): Path<i64>,

    user: AuthUser,

) -> AppResult<Json<Value>> {

    let _user_id = user.0;



    let mut rows = sqlx::query(

        r#"SELECT g.id, g.name, COALESCE(g.avatar_url, '') as avatar_url,

                  COALESCE(g.description, '') as description, g.owner_id,

                  DATE_FORMAT(g.created_at, '%Y-%m-%d %H:%i:%s') as created_at,

                  (SELECT COUNT(*) FROM group_members gm WHERE gm.group_id = g.id) as member_count

           FROM `groups` g WHERE g.id = ?"#,

    )

    .bind(group_id)

    // 统一使用 fetch_all：sqlx Any 驱动下 fetch_optional 空结果集不归还连接

    .fetch_all(&state.db.pool)

    .await

    .map_err(|e| AppError::Internal(format!("查询群组失败: {}", e)))?;

    let row = rows.pop().ok_or_else(|| AppError::NotFound("群组不存在".to_string()))?;



    Ok(Json(serde_json::json!({

        "id": row.try_get::<i64, _>("id").unwrap_or(0),

        "name": row.try_get::<String, _>("name").unwrap_or_default(),

        "avatar_url": row.try_get::<String, _>("avatar_url").unwrap_or_default(),

        "description": row.try_get::<String, _>("description").unwrap_or_default(),

        "owner_id": row.try_get::<i64, _>("owner_id").unwrap_or(0),

        "created_at": row.try_get::<String, _>("created_at").unwrap_or_default(),

        "member_count": row.try_get::<i64, _>("member_count").unwrap_or(0)

    })))

}



/// 申请加入群组

pub async fn join_group(

    State(state): State<AppState>,

    Path(group_id): Path<i64>,

    user: AuthUser,

) -> AppResult<Json<Value>> {

    let user_id = user.0;



    // 1. 群组必须存在

    let mut group_row = sqlx::query("SELECT id FROM `groups` WHERE id = ?")

        .bind(group_id)

        .fetch_all(&state.db.pool)

        .await

        .map_err(|e| AppError::Internal(format!("查询群组失败: {}", e)))?;

    if group_row.pop().is_none() {

        return Err(AppError::NotFound("群组不存在".to_string()));

    }



    // 2. 已在群中则拒绝（避免重复插入）

    // 注意：线上 group_members 表是 (group_id, user_id, role, joined_at) 联合主键结构，
    // 没有自增 id 列（与 models::GroupMember 不一致），因此这里不能用 SELECT id。
    let mut member_row = sqlx::query("SELECT user_id FROM group_members WHERE group_id = ? AND user_id = ?")

        .bind(group_id)

        .bind(user_id)

        .fetch_all(&state.db.pool)

        .await

        .map_err(|e| AppError::Internal(format!("查询群成员失败: {}", e)))?;

    if member_row.pop().is_some() {

        return Err(AppError::BadRequest("你已在该群中".to_string()));

    }



    // 3. 写库：加入群组

    let gid = group_id;

    let uid = user_id;

    state.db.spawn_write(move |pool| async move {

        let sql = "INSERT INTO group_members (group_id, user_id, role, joined_at) VALUES (?, ?, 'member', NOW())";

        sqlx::query(sql).bind(gid).bind(uid).execute(&pool).await.map_err(|e| e.to_string())?;

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("加入群组失败: {}", e)))?;



    tracing::info!("用户 {} 加入群组 {}", user_id, group_id);

    Ok(Json(serde_json::json!({ "message": "已加入群组" })))

}



/// 解散群组（仅群主）

pub async fn disband_group(

    State(state): State<AppState>,

    Path(group_id): Path<i64>,

    user: AuthUser,

) -> AppResult<Json<Value>> {

    let user_id = user.0;



    // 1. 群组存在性 + 群主校验

    let mut owner_row = sqlx::query("SELECT owner_id FROM `groups` WHERE id = ?")

        .bind(group_id)

        .fetch_all(&state.db.pool)

        .await

        .map_err(|e| AppError::Internal(format!("查询群组失败: {}", e)))?;

    let owner_id: i64 = match owner_row.pop() {

        Some(r) => r.try_get::<i64, _>("owner_id").unwrap_or(0),

        None => return Err(AppError::NotFound("群组不存在".to_string())),

    };

    if owner_id != user_id {

        return Err(AppError::Auth("只有群主可以解散群组".to_string()));

    }



    // 2. 写库：同一事务内删除成员关系与群组

    let gid = group_id;

    state.db.spawn_write(move |pool| async move {

        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM group_members WHERE group_id = ?")

            .bind(gid)

            .execute(&mut *tx)

            .await

            .map_err(|e| e.to_string())?;

        sqlx::query("DELETE FROM `groups` WHERE id = ?")

            .bind(gid)

            .execute(&mut *tx)

            .await

            .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;

        Ok(())

    }).await.map_err(|e| AppError::Internal(format!("解散群组失败: {}", e)))?;



    tracing::info!("群组 {} 已被群主 {} 解散", group_id, user_id);

    Ok(Json(serde_json::json!({ "message": "群组已解散" })))

}


