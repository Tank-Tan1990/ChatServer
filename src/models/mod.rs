//! 数据模型定义
//! ChatServer 核心数据结构

use serde::{Deserialize, Serialize};

/// 用户模型 - 日期用 String 避免 sqlx chrono 问题
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub nickname: String,
    #[sqlx(default)]
    pub avatar_url: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 用户注册请求
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub nickname: String,
}

/// 用户登录请求
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// 认证响应
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub refresh_token: String,
    pub user: UserInfo,
}

/// 用户信息（脱敏）
#[derive(Debug, Clone, Serialize)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub nickname: String,
    pub avatar_url: Option<String>,
    pub status: String,
}

impl From<User> for UserInfo {
    fn from(user: User) -> Self {
        UserInfo {
            id: user.id,
            username: user.username,
            nickname: user.nickname,
            avatar_url: user.avatar_url,
            status: user.status,
        }
    }
}

/// 好友关系
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Friend {
    pub id: i64,
    pub user_id: i64,
    pub friend_id: i64,
    pub status: String,
    pub created_at: String,
}

/// 消息模型 - MySQL 中 is_read 字段名对齐
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub sender_id: i64,
    pub receiver_id: Option<i64>,
    pub group_id: Option<i64>,
    pub content: String,
    pub msg_type: String,
    pub created_at: String,
    pub read: i32,
}

/// 发送消息请求
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub receiver_id: Option<i64>,
    pub group_id: Option<i64>,
    pub content: String,
    pub msg_type: Option<String>,
}

/// 群组模型
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Group {
    pub id: i64,
    pub name: String,
    #[sqlx(default)]
    pub avatar_url: Option<String>,
    pub owner_id: i64,
    pub created_at: String,
}

/// 创建群组请求
#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub avatar_url: Option<String>,
}

/// 群成员
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct GroupMember {
    pub id: i64,
    pub group_id: i64,
    pub user_id: i64,
    pub role: String,
    pub joined_at: String,
}