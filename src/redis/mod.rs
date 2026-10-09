//! 分布式 Redis 客户端模块
//! 用于分布式状态存储、缓存、消息队列

use redis::AsyncCommands;
use std::sync::Arc;

/// Redis 操作封装
#[derive(Clone)]
pub struct RedisOps {
    client: redis::Client,
    pool_size: usize,
}

impl RedisOps {
    pub fn new(url: &str, pool_size: usize) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(url)?;
        Ok(Self { client, pool_size })
    }

    /// 获取连接
    async fn conn(&self) -> Result<redis::aio::Connection, redis::RedisError> {
        self.client.get_async_connection().await
    }

    /// 设置字符串
    pub async fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        conn.set(key, value).await.map_err(|e| e.to_string())
    }

    /// 设置字符串（带过期时间）
    pub async fn setex(&self, key: &str, seconds: u64, value: &str) -> Result<(), String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        conn.set_ex(key, value, seconds).await.map_err(|e| e.to_string())
    }

    /// 获取字符串
    pub async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        let result: Option<String> = conn.get(key).await.map_err(|e| e.to_string())?;
        Ok(result)
    }

    /// 删除键
    pub async fn del(&self, key: &str) -> Result<bool, String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        let count: usize = conn.del(key).await.map_err(|e| e.to_string())?;
        Ok(count > 0)
    }

    /// 检查键是否存在
    pub async fn exists(&self, key: &str) -> Result<bool, String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        let result: bool = conn.exists(key).await.map_err(|e| e.to_string())?;
        Ok(result)
    }

    /// 推送到列表尾部（写队列入队用）
    pub async fn rpush(&self, key: &str, value: &str) -> Result<(), String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        conn.rpush(key, value).await.map_err(|e| e.to_string())
    }

    /// 从列表头部弹出（WriteWorker 消费用）
    pub async fn lpop(&self, key: &str) -> Result<Option<String>, String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        let result: Option<String> = conn.lpop(key, None).await.map_err(|e| e.to_string())?;
        Ok(result)
    }

    /// 批量从列表头部弹出（WriteWorker 批量消费）
    pub async fn lpop_batch(&self, key: &str, count: usize) -> Result<Vec<String>, String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        use std::num::NonZeroUsize;
        let result: Vec<String> = conn.lpop(key, NonZeroUsize::new(count)).await.map_err(|e| e.to_string())?;
        Ok(result)
    }

    /// 获取列表长度
    pub async fn llen(&self, key: &str) -> Result<usize, String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        let len: usize = conn.llen(key).await.map_err(|e| e.to_string())?;
        Ok(len)
    }

    /// 设置过期时间
    pub async fn expire(&self, key: &str, seconds: usize) -> Result<bool, String> {
        let mut conn = self.conn().await.map_err(|e| e.to_string())?;
        let result: bool = conn.expire(key, seconds as i64).await.map_err(|e| e.to_string())?;
        Ok(result)
    }

    // ===== 用户在线状态管理 =====

    /// 用户上线
    pub async fn user_online(&self, user_id: i64) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let info = serde_json::json!({
            "user_id": user_id,
            "connected_at": now,
            "last_heartbeat": now
        }).to_string();

        let key = format!("online_user:{}", user_id);
        self.setex(&key, 300, &info).await
    }

    /// 用户下线
    pub async fn user_offline(&self, user_id: i64) -> Result<(), String> {
        let key = format!("online_user:{}", user_id);
        self.del(&key).await.map(|_| ())
    }

    /// 检查用户是否在线
    pub async fn is_user_online(&self, user_id: i64) -> Result<bool, String> {
        let key = format!("online_user:{}", user_id);
        self.exists(&key).await
    }

    /// 更新用户心跳
    pub async fn update_heartbeat(&self, user_id: i64) -> Result<(), String> {
        let key = format!("online_user:{}", user_id);
        
        if let Some(json) = self.get(&key).await? {
            let mut info: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            info["last_heartbeat"] = serde_json::json!(now);
            
            self.setex(&key, 300, &info.to_string()).await
        } else {
            Err("用户不存在".to_string())
        }
    }
}

/// Redis 连接池
pub type RedisPool = Arc<RedisOps>;

/// 创建 Redis 连接池
pub async fn create_redis_pool(url: &str, pool_size: usize) -> Result<RedisPool, String> {
    let ops = RedisOps::new(url, pool_size).map_err(|e| e.to_string())?;
    Ok(Arc::new(ops))
}