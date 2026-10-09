//! Snowflake ID 生成器（单机简化版）
//!
//! 布局（64 bit 有符号整数，实际使用 63 bit）：
//! - 41 bit: 毫秒时间戳（相对自定义 epoch）
//! - 10 bit: worker_id（单机固定为 0，分布式部署需通过配置传入）
//! - 12 bit: 同一毫秒内的序列号
//!
//! 自定义 epoch: 2024-01-01 00:00:00 UTC

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const EPOCH_MS: u64 = 1704067200000; // 2024-01-01 00:00:00 UTC
const WORKER_ID_BITS: u8 = 10;
const SEQUENCE_BITS: u8 = 12;
const MAX_SEQUENCE: u16 = (1 << SEQUENCE_BITS) - 1;

pub struct Snowflake {
    worker_id: u16,
    state: Mutex<SnowflakeState>,
}

struct SnowflakeState {
    last_timestamp: u64,
    sequence: u16,
}

impl Snowflake {
    pub fn new(worker_id: u16) -> Self {
        let max_worker_id = (1 << WORKER_ID_BITS) - 1;
        assert!(
            worker_id <= max_worker_id,
            "worker_id 必须 <= {}",
            max_worker_id
        );
        Self {
            worker_id,
            state: Mutex::new(SnowflakeState {
                last_timestamp: 0,
                sequence: 0,
            }),
        }
    }

    pub fn next_id(&self) -> i64 {
        let mut state = self.state.lock().expect("Snowflake 锁未污染");
        loop {
            let now = current_ms();
            if now < state.last_timestamp {
                // 时钟回拨：等待 1ms 后重试（简化处理）
                drop(state);
                std::thread::sleep(std::time::Duration::from_millis(1));
                state = self.state.lock().expect("Snowflake 锁未污染");
                continue;
            }

            if now == state.last_timestamp {
                state.sequence = (state.sequence + 1) & MAX_SEQUENCE;
                if state.sequence == 0 {
                    // 序列溢出，等待下一毫秒
                    drop(state);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    state = self.state.lock().expect("Snowflake 锁未污染");
                    continue;
                }
            } else {
                state.sequence = 0;
            }

            state.last_timestamp = now;
            let id = ((now - EPOCH_MS) << (WORKER_ID_BITS + SEQUENCE_BITS))
                | ((self.worker_id as u64) << SEQUENCE_BITS)
                | (state.sequence as u64);
            return id as i64;
        }
    }
}

fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间早于 UNIX_EPOCH")
        .as_millis() as u64
}
