//! Message Queue Module - 消息队列持久化优化
//! Optimization 2: 高峰期消息先写入 Redis，再异步入库
//!
//! ⚠️ 状态说明（2026-09-01）：本模块**尚未接线**，`src/main.rs` 中没有 `mod msg_queue;` 声明，
//! 因此不参与编译、不参与运行。当前 `send_message` 走的是**直接写 MySQL** 的同步路径。
//! 若要启用本模块，必须先评估：Redis 不可用或刷库失败时消息会**丢失**（flush 失败仅打日志）。
//! 启用方式：在 main.rs 增加 `mod msg_queue;`，构造 MessageQueue 并在 send_message 中改为 enqueue。

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub id: String,
    pub sender_id: i64,
    pub receiver_id: Option<i64>,
    pub group_id: Option<i64>,
    pub content: String,
    pub msg_type: String,
    pub created_at: String,
}

const FILE_TYPES: &[&str] = &["image", "file", "audio", "video"];
const REDIS_QUEUE_KEY: &str = "msg_queue:pending";
const REDIS_QUEUE_STREAM: &str = "msg_queue:stream";

pub fn is_file_message(msg_type: &str) -> bool {
    FILE_TYPES.iter().any(|t| *t == msg_type)
}

pub struct MessageQueue {
    sender: mpsc::Sender<QueuedMessage>,
}

impl Clone for MessageQueue {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl MessageQueue {
    /// 创建消息队列
    /// buffer_size: mpsc channel 缓冲大小
    pub fn new(buffer_size: usize, redis_pool: crate::redis::RedisPool, pool: crate::db::DbPool) -> Self {
        let (sender, receiver) = mpsc::channel(buffer_size);
        let redis = redis_pool.clone();
        let p = pool.clone();
        tokio::spawn(async move { message_worker(receiver, redis, p).await; });
        tracing::info!("Message Queue started (Redis-backed), buffer_size={}", buffer_size);
        Self { sender }
    }
    
    /// 入队消息（先写 Redis，再异步刷库）
    pub async fn enqueue(&self, msg: QueuedMessage) -> Result<(), String> {
        self.sender.send(msg).await.map_err(|e| e.to_string())
    }
}

/// 生成唯一消息ID
fn generate_msg_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    format!("msg_{}_{}", now, rand_u64())
}

fn rand_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// 消息处理worker
async fn message_worker(
    mut receiver: mpsc::Receiver<QueuedMessage>, 
    redis: crate::redis::RedisPool,
    pool: crate::db::DbPool,
) {
    let db_pool = pool.clone(); // Owned copy for the loop
    let batch_size = 100;
    let flush_interval_ms = 500; // 每500ms强制刷新
    
    loop {
        tokio::select! {
            msg = receiver.recv() => {
                match msg {
                    Some(m) => {
                        // Optimization 2: 先写入 Redis 队列
                        if let Err(e) = push_to_redis(&redis, &m).await {
                            tracing::error!("Redis push failed: {}", e);
                        }
                    }
                    None => {
                        tracing::info!("Queue closed, flushing remaining messages...");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(flush_interval_ms)) => {
                // 定期从 Redis 刷入 MySQL
                if let Err(e) = flush_redis_to_mysql(&redis, &pool).await {
                    tracing::error!("Flush Redis->MySQL failed: {}", e);
                }
            }
        }
    }
    
    // 最后一次刷新
    let _ = flush_redis_to_mysql(&redis, &pool).await;
    tracing::info!("Message worker stopped");
}

/// 将消息推入 Redis 队列
async fn push_to_redis(redis: &crate::redis::RedisPool, msg: &QueuedMessage) -> Result<(), String> {
    let msg_json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    
    // 使用 LPUSH 存入列表（原子操作）
    redis.rpush(REDIS_QUEUE_KEY, &msg_json).await
        .map_err(|e| format!("redis rpush failed: {}", e))?;
    
    Ok(())
}

/// 从 Redis 批量读取并写入 MySQL
async fn flush_redis_to_mysql(redis: &crate::redis::RedisPool, pool: &crate::db::DbPool) -> Result<(), String> {
    let db_pool = pool;
    let mut success_count = 0;
    let mut error_count = 0;
    
    loop {
        // 使用 LPOP 弹出消息（RPUSH + LPOP = FIFO 顺序）
        let msg_json = match redis.lpop(REDIS_QUEUE_KEY).await {
            Ok(Some(json)) => json,
            Ok(None) => break, // 队列已空
            Err(e) => {
                tracing::warn!("Redis rpop failed: {}", e);
                break;
            }
        };
        
        // 解析消息
        let msg: QueuedMessage = match serde_json::from_str(&msg_json) {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Parse queued message failed: {}", e);
                error_count += 1;
                continue;
            }
        };
        
        // 写入 MySQL
        let result = sqlx::query(
            "INSERT INTO messages (sender_id, receiver_id, group_id, content, msg_type, created_at, is_read) VALUES (?, ?, ?, ?, ?, ?, 0)"
        )
        .bind(msg.sender_id)
        .bind(msg.receiver_id)
        .bind(msg.group_id)
        .bind(&msg.content)
        .bind(&msg.msg_type)
        .bind(&msg.created_at)
        .execute(&db_pool.pool)
        .await;
        
        match result {
            Ok(_) => success_count += 1,
            Err(e) => {
                error_count += 1;
                tracing::error!("DB write failed: {}", e);
                // 可选：将失败的消息放回 Redis 队列（简化处理，暂不实现）
            }
        }
    }
    
    if success_count > 0 {
        tracing::info!("Redis->MySQL flush: {} success, {} failed", success_count, error_count);
    }
    
    Ok(())
}

/// 获取队列待处理消息数量
pub async fn get_queue_size(redis: &crate::redis::RedisPool) -> Result<i64, String> {
    redis.llen(REDIS_QUEUE_KEY).await.map(|n| n as i64).map_err(|e| e.to_string())
}
