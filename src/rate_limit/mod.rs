//! 分布式限流模块
//! 基于 Redis 实现滑动窗口限流

use crate::redis::RedisPool;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// 限流配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub requests_per_minute: u32,
    pub burst_size: Option<u32>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_minute: 60,
            burst_size: Some(100),
        }
    }
}

/// 限流结果
#[derive(Debug, Clone, Serialize)]
pub struct RateLimitResult {
    pub allowed: bool,
    pub remaining: u32,
    pub reset_at: u64,
}

/// 分布式限流器
pub struct DistributedRateLimiter {
    redis: RedisPool,
    config: RateLimitConfig,
}

impl DistributedRateLimiter {
    pub fn new(redis: RedisPool, config: RateLimitConfig) -> Self {
        Self { redis, config }
    }

    /// 检查是否允许请求（滑动窗口算法）
    pub async fn check(&self, key: &str) -> Result<RateLimitResult, String> {
        if !self.config.enabled {
            return Ok(RateLimitResult {
                allowed: true,
                remaining: u32::MAX,
                reset_at: 0,
            });
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let window = 60_000; // 60秒窗口
        let _window_start = now - window;
        
        let redis_key = format!("ratelimit:{}:{}", key, now / 1000 / 60);

        // 使用 Redis 列表实现滑动窗口
        let count: usize = self.redis.llen(&redis_key).await
            .map_err(|e| e.to_string())?;

        let limit = self.config.requests_per_minute as usize;
        
        if count >= limit {
            // 计算重置时间
            let reset_at = (now / 1000 / 60 + 1) * 60 * 1000;
            return Ok(RateLimitResult {
                allowed: false,
                remaining: 0,
                reset_at,
            });
        }

        // 添加请求记录
        let _ = self.redis.rpush(&redis_key, &now.to_string()).await;
        
        // 设置过期时间（窗口结束后自动清理）
        let _ = self.redis.expire(&redis_key, 120).await;

        Ok(RateLimitResult {
            allowed: true,
            remaining: (limit - count - 1) as u32,
            reset_at: now + window,
        })
    }

    /// 检查并记录请求
    pub async fn allow(&self, key: &str) -> Result<bool, String> {
        Ok(self.check(key).await?.allowed)
    }
}

/// 简单限流器（内存版，用于开发/测试）
pub struct SimpleRateLimiter {
    config: RateLimitConfig,
    counters: std::collections::HashMap<String, (u64, u64)>, // (count, window_start)
}

impl SimpleRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            counters: std::collections::HashMap::new(),
        }
    }

    /// 检查请求
    pub fn check(&mut self, key: &str) -> RateLimitResult {
        if !self.config.enabled {
            return RateLimitResult {
                allowed: true,
                remaining: u32::MAX,
                reset_at: 0,
            };
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let window = 60_000;
        let _window_start = now - window;

        let entry = self.counters.entry(key.to_string()).or_insert((0, _window_start));
        
        // 窗口重置
        if entry.1 < _window_start {
            entry.0 = 0;
            entry.1 = now;
        }

        let limit = self.config.requests_per_minute as u64;
        
        if entry.0 >= limit {
            return RateLimitResult {
                allowed: false,
                remaining: 0,
                reset_at: now + window,
            };
        }

        entry.0 += 1;

        RateLimitResult {
            allowed: true,
            remaining: (limit - entry.0) as u32,
            reset_at: now + window,
        }
    }

    /// 检查并记录
    pub fn allow(&mut self, key: &str) -> bool {
        self.check(key).allowed
    }
}