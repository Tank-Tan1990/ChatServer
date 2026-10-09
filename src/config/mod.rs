//! 配置管理模块
//! 读取配置文件，支持 YAML/JSON 格式

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 应用配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub https: Option<HttpsConfig>,
    pub database: DatabaseConfig,
    pub jwt: JwtConfig,
    pub logging: LoggingConfig,
    pub rate_limit: RateLimitConfig,
}

/// 服务器配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: Option<usize>,
    pub keep_alive: u64,
}

/// HTTPS 配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpsConfig {
    pub enabled: bool,
    pub port: u16,
    pub cert_path: String,
    pub key_path: String,
}

/// 数据库配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub url: String,
    // 读连接池配置
    pub read_pool_max: u32,
    pub read_pool_min: Option<u32>,
    // 写连接池配置
    pub write_pool_max: u32,
    pub write_pool_min: Option<u32>,
    // 读写池总连接数上限（默认 800，不超过 MySQL max_connections 的 80%）
    pub max_total_connections: Option<u32>,
    // WriteWorker 配置
    pub write_worker_num: Option<u32>,
    pub worker_db_connections: Option<u32>,
}

/// JWT 配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JwtConfig {
    pub secret: String,
    pub refresh_secret: String,
    pub access_token_expire_minutes: u64,
    pub refresh_token_expire_days: u64,
}

/// 日志配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    pub level: String,
    pub file: Option<String>,
    pub format: String,
}

/// 限流配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub requests_per_minute: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8080,
                workers: None,
                keep_alive: 60,
            },
            https: None, // HTTPS 默认关闭，生产环境通过 config.yaml 启用
            database: DatabaseConfig {
                url: "mysql://chatuser:CHANGE_ME@127.0.0.1:3306/chat_server".to_string(),
                read_pool_max: 200,
                read_pool_min: Some(10),
                write_pool_max: 50,
                write_pool_min: Some(5),
                max_total_connections: Some(800),
                write_worker_num: Some(10),
                worker_db_connections: Some(5),
            },
            // ⚠️ 安全警告：默认密钥仅用于开发！生产环境必须通过 config.yaml 或环境变量覆盖
            jwt: JwtConfig {
                secret: std::env::var("JWT_SECRET").unwrap_or_else(|_| {
                    tracing::warn!("⚠️ 使用默认 JWT_SECRET，请设置 JWT_SECRET 环境变量！");
                    "change-me-jwt-secret-in-production".to_string()
                }),
                refresh_secret: std::env::var("JWT_REFRESH_SECRET").unwrap_or_else(|_| {
                    "change-me-refresh-secret-dev-only".to_string()
                }),
                access_token_expire_minutes: 120,  // 2小时
                refresh_token_expire_days: 7,
            },
            logging: LoggingConfig {
                level: "debug".to_string(),
                file: None,
                format: "json".to_string(),
            },
            rate_limit: RateLimitConfig {
                enabled: false,
                requests_per_minute: 60,
            },
        }
    }
}

/// 加载配置文件
pub fn load_config(config_path: Option<&str>) -> AppConfig {
    let config_path = config_path.unwrap_or("config.yaml");
    let path = PathBuf::from(config_path);

    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => {
                // 支持 YAML 和 JSON
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    serde_json::from_str(&content).unwrap_or_else(|e| {
                        tracing::warn!("配置文件解析失败: {}, 使用默认配置", e);
                        AppConfig::default()
                    })
                } else {
                    serde_yaml::from_str(&content).unwrap_or_else(|e| {
                        tracing::warn!("配置文件解析失败: {}, 使用默认配置", e);
                        AppConfig::default()
                    })
                }
            }
            Err(e) => {
                tracing::warn!("读取配置文件失败: {}, 使用默认配置", e);
                AppConfig::default()
            }
        }
    } else {
        tracing::info!("配置文件不存在, 使用默认配置");
        let default_config = AppConfig::default();
        // 保存默认配置供后续参考
        let _ = save_default_config(&path);
        default_config
    }
}

/// 保存默认配置文件
fn save_default_config(path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_yaml::to_string(&AppConfig::default())?;
    fs::write(path, content)?;
    tracing::info!("已生成默认配置文件: {:?}", path);
    Ok(())
}