//! 认证 API

//! 用户注册、登录、Token 验证、刷新 Token



use crate::error::{success_response, AppError, AppResult};

use crate::models::{AuthResponse, LoginRequest, RegisterRequest, User, UserInfo};

use crate::state::AppState;

use sqlx::Row;

use axum::{

    extract::State,

    Json,

};

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};

use serde::{Deserialize, Serialize};

use std::time::{SystemTime, UNIX_EPOCH};

use std::sync::OnceLock;
use std::sync::Arc;
use tokio::sync::Semaphore;

use sha2::{Sha256, Digest};

use rand::Rng;

use argon2::{
    password_hash::{rand_core::RngCore, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};



/// 密码哈希方案：新用户统一使用 Argon2id（PHC 字符串格式）。
/// 旧 SHA256 密码在登录验证通过后自动重哈希为 Argon2id（透明迁移）。
/// 同时保留 bcrypt 兼容分支，用于更早期的历史数据。
// Argon2id 参数：平衡安全性与 10K 并发性能。
// 默认使用 OWASP 2019 最低推荐 (m=7MiB, t=1, p=1)；
// 若环境变量 ARGON2_MODE=secure 则切回 OWASP 2023 推荐 (m=19MiB, t=2, p=1)。
fn argon2_params() -> argon2::Params {
    let secure = std::env::var("ARGON2_MODE").unwrap_or_default() == "secure";
    if secure {
        argon2::Params::new(19 * 1024, 2, 1, Some(32)).expect("Argon2 参数合法")
    } else {
        argon2::Params::new(7 * 1024, 1, 1, Some(32)).expect("Argon2 参数合法")
    }
}

fn argon2_hasher() -> Argon2<'static> {
    Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2_params(),
    )
}

/// 使用 Argon2id 计算密码哈希（CPU 密集型，应在 spawn_blocking 中调用）。
fn hash_password_argon2id(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    argon2_hasher().hash_password(password.as_bytes(), &salt).map(|ph| ph.to_string())
}

/// 验证密码，返回 (是否通过, 是否需要迁移)。
/// 需要迁移指当前哈希是旧 SHA256 或 bcrypt，应在登录成功后更新为 Argon2id。
fn verify_password(password: &str, hash_str: &str) -> Result<(bool, bool), AppError> {
    if hash_str.starts_with("$argon2id$") {
        let parsed = PasswordHash::new(hash_str)
            .map_err(|_| AppError::Auth("密码哈希格式错误".to_string()))?;
        let ok = argon2_hasher().verify_password(password.as_bytes(), &parsed).is_ok();
        return Ok((ok, false));
    }

    if hash_str.starts_with("sha256$") {
        let parts: Vec<&str> = hash_str.split('$').collect();
        if parts.len() != 3 {
            return Err(AppError::Auth("密码格式错误".to_string()));
        }
        let salt = parts[1];
        let stored_hash = parts[2];
        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        hasher.update(salt.as_bytes());
        let computed = hex::encode(hasher.finalize());
        return Ok((computed == stored_hash, computed == stored_hash));
    }

    // 兼容旧 bcrypt 密码
    let ok = bcrypt::verify(password, hash_str)
        .map_err(|_| AppError::Auth("密码验证失败".to_string()))?;
    Ok((ok, ok))
}



/// 登录密码验证并发控制信号量。
/// Argon2id verify 是 CPU 密集型，10K 并发同时 verify 会占满 CPU 导致大量超时。
/// 限制同时 verify 的数量（默认 300），让多余请求排队而非竞争 CPU。
fn login_verify_sem() -> Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| {
        let permits: usize = std::env::var("LOGIN_VERIFY_PERMITS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        Arc::new(Semaphore::new(permits))
    }).clone()
}

/// JWT Secret（从环境变量读取，默认值仅用于开发）

pub static JWT_SECRET: OnceLock<Vec<u8>> = OnceLock::new();

pub static JWT_REFRESH_SECRET: OnceLock<Vec<u8>> = OnceLock::new();



pub fn get_jwt_secret() -> &'static [u8] {

    JWT_SECRET.get_or_init(|| {

        std::env::var("JWT_SECRET")

            .unwrap_or_else(|_| "change-me-jwt-secret-in-production".to_string())

            .into_bytes()

    })

}



pub fn get_jwt_refresh_secret() -> &'static [u8] {

    JWT_REFRESH_SECRET.get_or_init(|| {

        std::env::var("JWT_REFRESH_SECRET")

            .unwrap_or_else(|_| "change-me-jwt-refresh-secret-in-production".to_string())

            .into_bytes()

    })

}



// 兼容旧代码的常量（已弃用，建议使用 get_jwt_secret()）

#[deprecated(note = "Use get_jwt_secret() instead")]

pub const JWT_SECRET_DEFAULT: &[u8] = b"change-me-jwt-secret-in-production";



const ACCESS_TOKEN_EXPIRE_HOURS: u64 = 2;

const REFRESH_TOKEN_EXPIRE_DAYS: u64 = 7;



/// JWT Payload

#[derive(Debug, Serialize, Deserialize)]

pub struct Claims {

    pub sub: String,

    pub exp: u64,

    pub iat: u64,

}



/// 用户注册

pub async fn register(

    State(state): State<AppState>,

    Json(req): Json<RegisterRequest>,

) -> AppResult<Json<AuthResponse>> {

    let username = req.username.trim().to_string();

    let nickname = if req.nickname.is_empty() { username.clone() } else { req.nickname.clone() };



    // 检查用户名是否已存在（读库，不阻塞）


    // ⚠️ 必须用 fetch_all，不能用 fetch_optional。
    // sqlx 0.7 的 Any 驱动在 fetch_optional 命中「空结果集」时不会把连接归还池，
    // 而注册查的正是「不存在的用户名」——即每次注册都漏 1 条读池连接，
    // 40 次之后读池耗尽、服务进入不可恢复状态（04-28 记载的正是这个现象）。
    let existing_rows: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM users WHERE username = ? LIMIT 1")
        .bind(&username)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询用户失败: {}", e)))?;


    if !existing_rows.is_empty() {

        return Err(AppError::BadRequest("用户名已存在".to_string()));

    }



    // Argon2id 哈希（CPU 密集型，走 spawn_blocking）。
    // 注意：spawn_blocking 闭包里只持有 password 字符串，不持有数据库连接，
    // 因此不会造成连接池泄漏。
    let password = req.password.clone();
    let password_hash = tokio::task::spawn_blocking(move || {
        hash_password_argon2id(&password)
            .map_err(|_| AppError::Internal("密码哈希失败".to_string()))
    })
    .await
    .map_err(|e| AppError::Internal(format!("密码哈希任务失败: {}", e)))??;



    // 写库：插入用户

    let username_ins = username.clone();

    let hash_ins = password_hash.clone();

    let nickname_ins = nickname.clone();
    let user_id: i64 = state.db.spawn_write(move |pool| async move {
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        
        let sql = "INSERT INTO users (username, password_hash, nickname, status, created_at, updated_at) VALUES (?, ?, ?, 'offline', NOW(), NOW())";
        sqlx::query(sql)
            .bind(&username_ins)
            .bind(&hash_ins)
            .bind(&nickname_ins)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        let row = sqlx::query("SELECT LAST_INSERT_ID() as id")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        
        let id: i64 = row.try_get::<i64, _>("id").unwrap_or(0);
        
        tx.commit().await.map_err(|e| e.to_string())?;
        
        Ok::<i64, String>(id)
    }).await.map_err(|e| AppError::Internal(format!("注册失败: {}", e)))?;


    if user_id == 0 {

        return Err(AppError::Internal("注册失败：未获取到用户ID".to_string()));

    }



    let (access_token, refresh_token) = generate_tokens(user_id)?;



    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let user = User {

        id: user_id,

        username,

        password_hash,

        nickname,

        avatar_url: None,

        status: "offline".to_string(),

        created_at: now.clone(),

        updated_at: now,

    };



    tracing::info!("用户注册成功: {}", user.username);



    Ok(Json(AuthResponse {

        token: access_token,

        refresh_token,

        user: user.into(),

    }))

}



/// 用户登录

pub async fn login(

    State(state): State<AppState>,

    Json(req): Json<LoginRequest>,

) -> AppResult<Json<AuthResponse>> {


    // 同样避开 fetch_optional 的空结果集缺陷（用户名不存在时返回 None 会漏连接）
    let mut user_rows: Vec<User> = sqlx::query_as("SELECT id, username, password_hash, nickname, COALESCE(avatar_url, '') as avatar_url, status, DATE_FORMAT(created_at,'%Y-%m-%d %H:%i:%s') as created_at, DATE_FORMAT(updated_at,'%Y-%m-%d %H:%i:%s') as updated_at FROM users WHERE username = ?")
        .bind(&req.username)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询用户失败: {}", e)))?;

    let user = user_rows.pop()
        .ok_or_else(|| AppError::Auth("用户名或密码错误".to_string()))?;




    // 密码验证：优先 Argon2id，兼容 SHA256，兜底 bcrypt。
    // SHA256 / bcrypt 旧密码验证通过后异步重哈希为 Argon2id。
    // 用信号量限制同时做 Argon2id verify 的并发数，避免 CPU 被占满导致大规模超时。
    let password = req.password.clone();
    let hash_str = user.password_hash.clone();

    let (valid, needs_rehash) = {
        let _permit = login_verify_sem()
            .acquire_owned()
            .await
            .map_err(|_| AppError::Internal("登录验证服务不可用".to_string()))?;
        // Argon2id verify 是 CPU 密集型，必须放在 spawn_blocking 中避免阻塞 tokio worker。
        let pwd = password.clone();
        tokio::task::spawn_blocking(move || verify_password(&pwd, &hash_str))
            .await
            .map_err(|e| AppError::Internal(format!("密码验证任务失败: {}", e)))??
    };
    if !valid {
        return Err(AppError::Auth("用户名或密码错误".to_string()));
    }

    // 写库：更新用户状态；若为旧哈希则顺带迁移为 Argon2id。
    // 10K 并发场景下，同步等待 UPDATE 会被 MySQL innodb_flush_log_at_trx_commit=1 拖垮，
    // 因此将状态更新与密码迁移改为后台 fire-and-forget，不阻塞登录响应。
    let user_id = user.id;
    let new_hash = if needs_rehash {
        match tokio::task::spawn_blocking(move || hash_password_argon2id(&password)).await {
            Ok(Ok(h)) => Some(h),
            Ok(Err(_)) => None,
            Err(_) => None,
        }
    } else {
        None
    };

    let write_pool = state.db.write_pool.clone();
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE users SET status = 'online' WHERE id = ?")
            .bind(user_id)
            .execute(&write_pool)
            .await;
        if let Some(h) = new_hash {
            let _ = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
                .bind(h)
                .bind(user_id)
                .execute(&write_pool)
                .await;
        }
    });



    let (access_token, refresh_token) = generate_tokens(user.id)?;

    tracing::info!("用户登录成功: {}", req.username);



    let mut user = user;

    user.status = "online".to_string();



    Ok(Json(AuthResponse {

        token: access_token,

        refresh_token,

        user: user.into(),

    }))

}



/// 刷新 Token

#[derive(Debug, Deserialize)]

pub struct RefreshRequest {

    pub refresh_token: String,

}



pub async fn refresh_token(

    State(state): State<AppState>,

    Json(req): Json<RefreshRequest>,

) -> AppResult<Json<serde_json::Value>> {

    let claims = decode_token(&req.refresh_token, get_jwt_refresh_secret())?;

    let user_id: i64 = claims.sub.parse()

        .map_err(|_| AppError::Auth("无效的 Token".to_string()))?;



    let mut exist_rows: Vec<User> = sqlx::query_as("SELECT id, username, password_hash, nickname, COALESCE(avatar_url, '') as avatar_url, status, DATE_FORMAT(created_at,'%Y-%m-%d %H:%i:%s') as created_at, DATE_FORMAT(updated_at,'%Y-%m-%d %H:%i:%s') as updated_at FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询用户失败: {}", e)))?;

    let _ = exist_rows.pop()
        .ok_or_else(|| AppError::NotFound("用户不存在".to_string()))?;



    let (access_token, new_refresh_token) = generate_tokens(user_id)?;



    Ok(success_response(serde_json::json!({

        "token": access_token,

        "refresh_token": new_refresh_token

    })))

}



/// 获取当前用户信息

#[derive(Debug, Deserialize)]

pub struct TokenQuery {

    pub token: Option<String>,

}



pub async fn get_user_info(

    user: crate::api::extractors::AuthUser,

    State(state): State<AppState>,

) -> AppResult<Json<UserInfo>> {

    let user_id = user.0;



    let mut user_rows: Vec<User> = sqlx::query_as("SELECT id, username, password_hash, nickname, COALESCE(avatar_url, '') as avatar_url, status, DATE_FORMAT(created_at,'%Y-%m-%d %H:%i:%s') as created_at, DATE_FORMAT(updated_at,'%Y-%m-%d %H:%i:%s') as updated_at FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| AppError::Internal(format!("查询用户失败: {}", e)))?;

    let user = user_rows.pop()
        .ok_or_else(|| AppError::NotFound("用户不存在".to_string()))?;

    Ok(Json(user.into()))

}



/// 用户登出

pub async fn logout(

    user: crate::api::extractors::AuthUser,

    State(state): State<AppState>,

) -> AppResult<Json<serde_json::Value>> {

    let user_id = user.0;



    // 写库：更新用户状态

    let uid = user_id;
    state.db.spawn_write(move |pool| async move {
        sqlx::query("UPDATE users SET status = 'offline' WHERE id = ?")
            .bind(uid)
            .execute(&pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| AppError::Internal(format!("更新状态失败: {}", e)))?;



    tracing::info!("用户登出成功: {}", user_id);

    Ok(success_response(serde_json::json!({ "message": "已登出" })))

}



// ===== 辅助函数 =====



fn current_timestamp() -> u64 {

    SystemTime::now()

        .duration_since(UNIX_EPOCH)

        .map(|d| d.as_secs())

        .unwrap_or(0)

}



/// 临时诊断：打印连接池快照，用于定位连接泄漏发生在哪一步。
/// 定位完成后可连同 register/其它函数里的调用一并删除。
#[allow(dead_code)]
fn pool_snapshot(tag: &str, db: &crate::db::DbPool) {
    let rs = db.read_pool.size();
    let ri = db.read_pool.num_idle();
    let ws = db.write_pool.size();
    let wi = db.write_pool.num_idle();
    tracing::warn!(
        "POOLSTAT {}: read(size={}, idle={}, in_use={}) | write(size={}, idle={}, in_use={})",
        tag, rs, ri, rs.saturating_sub(ri as u32),
        ws, wi, ws.saturating_sub(wi as u32)
    );
}

fn generate_tokens(user_id: i64) -> AppResult<(String, String)> {

    let access_token = generate_token(user_id, get_jwt_secret(), ACCESS_TOKEN_EXPIRE_HOURS * 3600)?;

    let refresh_token = generate_token(user_id, get_jwt_refresh_secret(), REFRESH_TOKEN_EXPIRE_DAYS * 86400)?;

    Ok((access_token, refresh_token))

}



fn generate_token(user_id: i64, secret: &[u8], expire_seconds: u64) -> AppResult<String> {

    let now = current_timestamp();

    let claims = Claims {

        sub: user_id.to_string(),

        exp: now + expire_seconds,

        iat: now,

    };

    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret))

        .map_err(|e| AppError::Internal(format!("生成 Token 失败: {}", e)))

}



/// 解码 JWT Token（公开方法）

pub fn decode_token(token: &str, secret: &[u8]) -> AppResult<Claims> {

    decode::<Claims>(

        token,

        &DecodingKey::from_secret(secret),

        &Validation::default(),

    )

    .map(|data| data.claims)

    .map_err(|e| AppError::Auth(format!("Token 验证失败: {}", e)))

}



/// 从 Token 提取用户 ID（公开方法）

pub fn extract_user_id(token: &str) -> AppResult<i64> {

    let claims = decode_token(token, get_jwt_secret())?;

    claims.sub.parse()

        .map_err(|_| AppError::Auth("无效的 Token".to_string()))

}


