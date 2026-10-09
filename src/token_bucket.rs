//! 全局令牌桶限流 - 保护 MySQL 免受突发流量冲击
//! 基于 Redis 实现分布式令牌桶

use crate::redis::RedisPool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 全局令牌桶配置
#[derive(Debug, Clone)]
pub struct TokenBucketConfig {
    /// 每秒填充令牌数（QPS 上限）
    pub rate_per_second: u32,
    /// 桶容量（突发流量缓冲）
    pub capacity: u32,
}

impl Default for TokenBucketConfig {
    fn default() -> Self {
        // 默认 500 QPS，适配 MySQL innodb_thread_concurrency=25
        // 25 线程 * 20ms 平均处理时间 ≈ 1250 QPS 理论值
        // 保守设置 500 QPS，确保稳定性
        Self {
            rate_per_second: 500,
            capacity: 1000,
        }
    }
}

/// 全局令牌桶限流器
pub struct GlobalTokenBucket {
    redis: RedisPool,
    config: TokenBucketConfig,
    key: String,
}

impl GlobalTokenBucket {
    pub fn new(redis: RedisPool, config: TokenBucketConfig) -> Self {
        Self {
            redis,
            config,
            key: "global_token_bucket".to_string(),
        }
    }

    /// 尝试获取一个令牌
    /// 返回 true 表示允许请求，false 表示限流
    pub async fn acquire(&self) -> Result<bool, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as f64;
        
        let rate = self.config.rate_per_second as f64;
        let capacity = self.config.capacity as f64;
        
        // Lua 脚本实现令牌桶算法（原子操作）
        let lua_script = r#"
            local key = KEYS[1]
            local now = tonumber(ARGV[1])
            local rate = tonumber(ARGV[2])
            local capacity = tonumber(ARGV[3])
            
            -- 获取当前状态
            local tokens = redis.call('GET', key .. ':tokens')
            local last_time = redis.call('GET', key .. ':last_time')
            
            if tokens == false then
                tokens = capacity
                last_time = now
            else
                tokens = tonumber(tokens)
                last_time = tonumber(last_time)
            end
            
            -- 计算新增令牌
            local elapsed = now - last_time
            local new_tokens = math.min(capacity, tokens + elapsed * rate)
            
            -- 尝试获取令牌
            if new_tokens >= 1 then
                new_tokens = new_tokens - 1
                redis.call('SET', key .. ':tokens', new_tokens)
                redis.call('SET', key .. ':last_time', now)
                redis.call('EXPIRE', key .. ':tokens', 60)
                redis.call('EXPIRE', key .. ':last_time', 60)
                return 1
            else
                redis.call('SET', key .. ':tokens', new_tokens)
                redis.call('SET', key .. ':last_time', now)
                redis.call('EXPIRE', key .. ':tokens', 60)
                redis.call('EXPIRE', key .. ':last_time', 60)
                return 0
            end
        "#;
        
        // 使用 eval 执行 Lua 脚本
        let result: i32 = redis::cmd("EVAL")
            .arg(lua_script)
            .arg(1)  // numkeys
            .arg(&self.key)
            .arg(now.to_string())
            .arg(rate.to_string())
            .arg(capacity.to_string())
            .query_async(&mut self.redis.conn().await.map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;
        
        Ok(result == 1)
    }

    /// 获取当前令牌数（用于监控）
    pub async fn get_tokens(&self) -> Result<f64, String> {
        match self.redis.get(&format!("{}:tokens", self.key)).await? {
            Some(v) => v.parse().map_err(|e| format!("Parse error: {}", e)),
            None => Ok(self.config.capacity as f64),
        }
    }
}

/// 创建全局限流器
pub fn create_global_rate_limiter(redis: RedisPool, qps: u32) -> Arc<GlobalTokenBucket> {
    Arc::new(GlobalTokenBucket::new(
        redis,
        TokenBucketConfig {
            rate_per_second: qps,
            capacity: qps * 2, // 2 秒突发缓冲
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_token_bucket_config() {
        let config = TokenBucketConfig::default();
        assert_eq!(config.rate_per_second, 500);
        assert_eq!(config.capacity, 1000);
    }
}
