//! 分布式应用状态管理
//! 支持多实例部署，使用 Redis 共享状态

use crate::db::DbPool;
use crate::message_writer::AsyncMessageWriter;
use crate::redis::RedisPool;
use crate::snowflake::Snowflake;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 单个用户的多个设备连接：conn_id -> 该连接专属的广播通道
/// （同一用户手机+电脑同时在线，每台设备各自订阅自己的通道，互不顶替）
pub type UserConnections = HashMap<u64, broadcast::Sender<String>>;

/// 应用共享状态 - 分布式版本
#[derive(Clone)]
pub struct AppState {
    /// 数据库连接池（包含读写池和写信号量）
    pub db: DbPool,
    /// Redis 连接池（分布式状态）
    pub redis: RedisPool,
    /// 本地 WebSocket 在线用户（uid -> 该用户的全部设备连接）
    pub local_ws_users: Arc<RwLock<HashMap<i64, UserConnections>>>,
    /// 限流配置（env RATE_LIMIT_RPM 优先，config.yaml rate_limit 兜底）
    pub rate_limit_rpm: u32,
    /// 分布式锁配置
    pub lock_timeout: u64,
    /// Snowflake ID 生成器
    pub snowflake: Arc<Snowflake>,
    /// 消息异步批量写入器（可选）
    pub async_message_writer: Option<AsyncMessageWriter>,
}

impl AppState {
    pub fn new(db: DbPool, redis: RedisPool) -> Self {
        let snowflake = Arc::new(Snowflake::new(0));
        let async_message_writer = if std::env::var("MESSAGE_ASYNC").unwrap_or_default() == "1" {
            tracing::info!("✉️ 消息异步批量写入已启用（MESSAGE_ASYNC=1）");
            Some(AsyncMessageWriter::new(db.clone()))
        } else {
            None
        };
        Self {
            db,
            redis,
            local_ws_users: Arc::new(RwLock::new(HashMap::new())),
            rate_limit_rpm: 100_000,
            lock_timeout: 30,
            snowflake,
            async_message_writer,
        }
    }

    pub fn with_rate_limit(mut self, rpm: u32) -> Self {
        self.rate_limit_rpm = rpm;
        self
    }
}

// ===== 在线用户管理（分布式） =====

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OnlineUserInfo {
    pub user_id: i64,
    pub instance_id: String,
    pub connected_at: u64,
    pub last_heartbeat: u64,
}

impl AppState {
    /// 用户上线
    pub async fn user_online(&self, user_id: i64) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let info = OnlineUserInfo {
            user_id,
            instance_id: get_instance_id(),
            connected_at: now,
            last_heartbeat: now,
        };

        let key = format!("online_user:{}", user_id);
        let json = serde_json::to_string(&info).map_err(|e| e.to_string())?;
        self.redis.setex(&key, 300, &json).await
    }

    /// 用户下线
    pub async fn user_offline(&self, user_id: i64) -> Result<(), String> {
        let key = format!("online_user:{}", user_id);
        self.redis.del(&key).await.map(|_| ())
    }

    /// 检查用户是否在线
    pub async fn is_user_online(&self, user_id: i64) -> Result<bool, String> {
        let key = format!("online_user:{}", user_id);
        self.redis.exists(&key).await
    }

    /// 更新用户心跳
    pub async fn update_heartbeat(&self, user_id: i64) -> Result<(), String> {
        let key = format!("online_user:{}", user_id);
        
        if let Some(json) = self.redis.get(&key).await? {
            let mut info: OnlineUserInfo = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            info.last_heartbeat = now;
            
            let json = serde_json::to_string(&info).map_err(|e| e.to_string())?;
            self.redis.setex(&key, 300, &json).await
        } else {
            Err("用户不存在".to_string())
        }
    }
}

/// 获取实例 ID
fn get_instance_id() -> String {
    format!("{}-{}", 
        hostname::get().unwrap_or_default().to_string_lossy(),
        std::process::id()
    )
}