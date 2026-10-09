//! 消息异步批量写入模块
//!
//! 当环境变量 MESSAGE_ASYNC=1 时，send_message 将消息放入内存队列，
//! 由后台 worker 每 50ms 或积累 500 条批量 INSERT 到 MySQL。
//! 这能绕过 innodb_flush_log_at_trx_commit=1 下单条 fsync 的瓶颈，
//! 大幅提升高并发写入 QPS。
//!
//! ⚠️ 代价：
//! - 消息落库有最大 50ms 延迟；
//! - 进程崩溃时未 flush 的消息会丢失；
//! - 返回的 msg_id 由 Snowflake 生成，不再使用 MySQL 自增 ID。

use crate::db::DbPool;
use crate::models::Message;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{interval, Duration};

const CHANNEL_SIZE: usize = 200000;
const BATCH_SIZE: usize = 500;
const FLUSH_INTERVAL_MS: u64 = 50;
const WORKER_COUNT: usize = 3;

#[derive(Clone)]
pub struct AsyncMessageWriter {
    pub tx: mpsc::Sender<Message>,
}

impl AsyncMessageWriter {
    pub fn new(pool: DbPool) -> Self {
        let (tx, rx) = mpsc::channel::<Message>(CHANNEL_SIZE);
        // 多个 worker 共享同一个 receiver，提高 flush 并行度
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..WORKER_COUNT {
            let rx_clone = rx.clone();
            let pool_clone = pool.clone();
            tokio::spawn(async move {
                message_batch_worker(rx_clone, pool_clone).await;
                tracing::info!("消息批量写入 worker {} 已停止", i);
            });
        }
        Self { tx }
    }
}

async fn message_batch_worker(rx: Arc<Mutex<mpsc::Receiver<Message>>>, pool: DbPool) {
    let mut batch: Vec<Message> = Vec::with_capacity(BATCH_SIZE);
    let mut tick = interval(Duration::from_millis(FLUSH_INTERVAL_MS));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                if !batch.is_empty() {
                    flush_batch(&batch, &pool).await;
                    batch.clear();
                }
            }
            result = async {
                let mut receiver = rx.lock().await;
                receiver.recv().await
            } => {
                match result {
                    Some(msg) => {
                        batch.push(msg);
                        if batch.len() >= BATCH_SIZE {
                            flush_batch(&batch, &pool).await;
                            batch.clear();
                        }
                    }
                    None => break,
                }
            }
        }
    }

    if !batch.is_empty() {
        flush_batch(&batch, &pool).await;
    }
}

async fn flush_batch(batch: &[Message], pool: &DbPool) {
    if batch.is_empty() {
        return;
    }

    let mut tx = match pool.write_pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("消息批量写入 BEGIN 失败: {}", e);
            return;
        }
    };

    let mut ok = 0;
    let mut failed = 0;
    for msg in batch {
        let res = sqlx::query(
            "INSERT INTO messages (id, sender_id, receiver_id, group_id, content, msg_type, is_read, created_at) VALUES (?, ?, ?, ?, ?, ?, 0, NOW())"
        )
        .bind(msg.id)
        .bind(msg.sender_id)
        .bind(msg.receiver_id)
        .bind(msg.group_id)
        .bind(&msg.content)
        .bind(&msg.msg_type)
        .execute(&mut *tx)
        .await;

        match res {
            Ok(_) => ok += 1,
            Err(e) => {
                failed += 1;
                if failed <= 3 {
                    tracing::error!("消息批量写入单条失败: {}", e);
                }
            }
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("消息批量写入 COMMIT 失败: {}", e);
    } else {
        tracing::debug!("消息批量写入: {} 成功, {} 失败", ok, failed);
    }
}
