//! Circuit Breaker Module - 熔断器优化
//! Optimization 4: 当错误率超过阈值时自动熔断

use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::sync::{Arc, RwLock};


use std::time::{Duration, Instant};

/// 熔断器状态
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    /// 关闭状态：正常请求
    Closed,
    /// 半开状态：尝试恢复
    HalfOpen,
    /// 开启状态：拒绝请求
    Open,
}

/// 熔断器配置
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// 失败阈值（连续失败次数）
    pub failure_threshold: u64,
    /// 恢复超时（秒）
    pub recovery_timeout: u64,
    /// 半开状态成功阈值
    pub half_open_success_threshold: u64,
    /// 请求量阈值（低于此值不触发熔断）
    pub min_requests: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: 30,
            half_open_success_threshold: 3,
            min_requests: 10,
        }
    }
}

/// Circuit Breaker 实现
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: AtomicBool,  // true = open, false = closed/half-open
    half_open_successes: AtomicU64,
    consecutive_failures: AtomicU64,
    total_requests: AtomicU64,
    total_failures: AtomicU64,
    last_failure_time: RwLock<Option<Instant>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: AtomicBool::new(false),  // 默认关闭
            half_open_successes: AtomicU64::new(0),
            consecutive_failures: AtomicU64::new(0),
            total_requests: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
            last_failure_time: RwLock::new(None),
        }
    }

    /// 检查是否允许请求
    pub fn is_allowed(&self) -> bool {
        let state = self.get_state();
        match state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,  // 半开时允许部分请求
            CircuitState::Open => {
                // 检查是否超时
                let timeout = Duration::from_secs(self.config.recovery_timeout);
                if let Some(last_failure) = self.last_failure_time.read().ok().and_then(|g| *g) {
                    if last_failure.elapsed() >= timeout {
                        // 超时，进入半开状态
                        self.to_half_open();
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    /// 记录成功
    pub fn record_success(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        
        // 半开状态下，累积成功次数
        if self.get_state() == CircuitState::HalfOpen {
            let successes = self.half_open_successes.fetch_add(1, Ordering::Relaxed) + 1;
            if successes >= self.config.half_open_success_threshold {
                self.to_closed();
            }
        }
    }

    /// 记录失败
    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.total_failures.fetch_add(1, Ordering::Relaxed);
        
        // 更新最后失败时间
        *self.last_failure_time.write().unwrap() = Some(Instant::now());
        
        // 检查是否需要熔断
        if failures >= self.config.failure_threshold {
            self.to_open();
        }
    }

    /// 获取当前状态
    pub fn get_state(&self) -> CircuitState {
        if self.state.load(Ordering::Relaxed) {
            CircuitState::Open
        } else {
            CircuitState::Closed
        }
    }

    /// 进入熔断状态
    fn to_open(&self) {
        tracing::warn!(
            "Circuit Breaker OPEN: consecutive_failures={}, threshold={}",
            self.consecutive_failures.load(Ordering::Relaxed),
            self.config.failure_threshold
        );
        self.state.store(true, Ordering::Relaxed);
        self.half_open_successes.store(0, Ordering::Relaxed);
    }

    /// 进入半开状态
    fn to_half_open(&self) {
        tracing::info!("Circuit Breaker HALF-OPEN: recovery timeout reached");
        self.state.store(false, Ordering::Relaxed);
        self.half_open_successes.store(0, Ordering::Relaxed);
    }

    /// 进入关闭状态（恢复）
    fn to_closed(&self) {
        tracing::info!(
            "Circuit Breaker CLOSED: recovered after {} half-open successes",
            self.half_open_successes.load(Ordering::Relaxed)
        );
        self.state.store(false, Ordering::Relaxed);
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.half_open_successes.store(0, Ordering::Relaxed);
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> CircuitBreakerStats {
        let total = self.total_requests.load(Ordering::Relaxed);
        let failures = self.total_failures.load(Ordering::Relaxed);
        let error_rate = if total > 0 {
            (failures as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        
        CircuitBreakerStats {
            state: self.get_state(),
            total_requests: total,
            total_failures: failures,
            error_rate,
            consecutive_failures: self.consecutive_failures.load(Ordering::Relaxed),
            half_open_successes: self.half_open_successes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub state: CircuitState,
    pub total_requests: u64,
    pub total_failures: u64,
    pub error_rate: f64,
    pub consecutive_failures: u64,
    pub half_open_successes: u64,
}

/// HTTP API 路由用的熔断器包装
pub struct HttpCircuitBreaker {
    inner: Arc<CircuitBreaker>,
}

impl HttpCircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            inner: Arc::new(CircuitBreaker::new(config)),
        }
    }

    pub fn check(&self) -> Result<(), CircuitBreakerError> {
        if !self.inner.is_allowed() {
            return Err(CircuitBreakerError::CircuitOpen);
        }
        Ok(())
    }

    pub fn success(&self) {
        self.inner.record_success();
    }

    pub fn failure(&self) {
        self.inner.record_failure();
    }

    pub fn stats(&self) -> CircuitBreakerStats {
        self.inner.get_stats()
    }
}

#[derive(Debug, Clone)]
pub enum CircuitBreakerError {
    CircuitOpen,
}

impl std::fmt::Display for CircuitBreakerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitBreakerError::CircuitOpen => {
                write!(f, "Circuit breaker is open, service temporarily unavailable")
            }
        }
    }
}
