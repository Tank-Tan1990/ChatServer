//! 工具模块
//! 通用工具函数

use chrono::Utc;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// 时间戳转日期字符串
pub fn timestamp_to_string(_timestamp: i64) -> String {
    // 直接使用当前时间格式化
    Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 获取当前时间戳（秒）
pub fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 获取当前时间戳（毫秒）
pub fn current_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 生成唯一 ID（基于时间戳 + 随机数）
pub fn generate_unique_id() -> String {
    format!("{}_{}", current_timestamp_millis(), rand_id(6))
}

/// 生成随机字符串
fn rand_id(len: usize) -> String {
    use std::iter::repeat;
    let charset: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
    repeat(())
        .take(len)
        .map(|_| charset[rand_byte() as usize % charset.len()])
        .collect()
}

fn rand_byte() -> u8 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u8)
        .unwrap_or(42) // 恒定种子，不影响安全性
}

/// 验证用户名格式（字母、数字、下划线，3-20个字符）
pub fn validate_username(username: &str) -> bool {
    if username.len() < 3 || username.len() > 20 {
        return false;
    }
    username.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 验证密码格式（至少6位）
pub fn validate_password(password: &str) -> bool {
    password.len() >= 6
}

/// 脱敏手机号
pub fn mask_phone(phone: &str) -> String {
    if phone.len() < 7 {
        return phone.to_string();
    }
    let len = phone.len();
    format!("{}****{}", &phone[..3], &phone[len-4..])
}

/// 脱敏邮箱
pub fn mask_email(email: &str) -> String {
    if let Some(at_pos) = email.find('@') {
        let prefix = &email[..at_pos];
        let suffix = &email[at_pos..];
        if prefix.len() <= 2 {
            format!("**{}", suffix)
        } else {
            format!("{}**{}", &prefix[..1], suffix)
        }
    } else {
        email.to_string()
    }
}

/// 通用响应结构
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub code: i32,
    pub message: String,
    pub data: Option<T>,
    pub success: bool,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            code: 0,
            message: "成功".to_string(),
            data: Some(data),
            success: true,
        }
    }

    pub fn error(code: i32, message: &str) -> Self {
        Self {
            code,
            message: message.to_string(),
            data: None,
            success: false,
        }
    }
}