//! 数据库初始化与管理
//! 模块化数据库驱动，支持 MySQL, PostgreSQL, SQLite, SQL Server, 达梦, 人大金仓
//!
//! 架构说明：
//! - DbPool 是统一的连接池包装器，内部持有 sqlx::AnyPool
//! - SqlDialect trait 提供各数据库的 SQL 方言差异
//! - 通过 DATABASE_URL 环境变量自动识别数据库类型
//! - 各数据库的建表 SQL 独立维护，保证最佳兼容性

use sqlx::any::AnyPoolOptions;
use sqlx::Row;
use std::future::Future;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Semaphore};

use crate::config::AppConfig;

mod driver;
mod query_builder;

pub use driver::{DatabaseType, SqlDialect, create_dialect};
pub use query_builder::{QueryBuilder, InsertBuilder};

/// 统一数据库连接池
#[derive(Clone)]
pub struct DbPool {
    /// 底层连接池（sqlx AnyPool，支持多数据库）
    pub pool: sqlx::AnyPool,
    /// 数据库类型
    pub db_type: DatabaseType,
    /// SQL 方言（用于生成兼容 SQL）
    pub dialect: Arc<dyn SqlDialect>,
    /// 读连接池（SELECT 操作）
    pub read_pool: sqlx::AnyPool,
    /// 写连接池（INSERT/UPDATE/DELETE 操作，预创建）
    pub write_pool: sqlx::AnyPool,
    /// 写信号量（控制并发写）
    pub write_sem: Arc<Semaphore>,
    /// 写数据库 URL
    pub write_url: String,
    /// 读连接池最大连接数
    pub read_pool_max: u32,
    /// 写并发上限
    pub write_sem_size: u32,
}

impl DbPool {
    /// 获取 SQL 方言
    pub fn dialect(&self) -> &dyn SqlDialect {
        self.dialect.as_ref()
    }

    /// 获取数据库类型
    pub fn db_type(&self) -> DatabaseType {
        self.db_type
    }

    /// 获取最后插入的ID
    pub async fn last_insert_id(&self) -> Result<i64, String> {
        let sql = match self.db_type {
            DatabaseType::MySQL | DatabaseType::Dameng | DatabaseType::Kingbase =>
                "SELECT LAST_INSERT_ID() as id",
            DatabaseType::SQLite =>
                "SELECT last_insert_rowid() as id",
            _ => "SELECT 0 as id",
        };
        let row = sqlx::query(sql).fetch_one(&self.pool).await.map_err(|e| e.to_string())?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        Ok(id)
    }

    /// 执行一个带并发控制的写操作（异步方案）
    /// 接收闭包 |pool| async move { ... }，返回 Future
    pub async fn spawn_write<R, F, Fut>(
        &self,
        f: F,
    ) -> Result<R, String>
    where
        R: Send + 'static,
        F: FnOnce(sqlx::AnyPool) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R, String>> + Send + 'static,
    {
        // 信号量限制并发写数量
        let sem = self.write_sem.clone();
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            sem.acquire_owned(),
        )
        .await
        .map_err(|_| "写操作超时，请稍后重试".to_string())?
        .map_err(|_| "写服务暂时不可用".to_string())?;

        // 写操作必须走 write_pool：信号量按 write_pool 的容量放行并发，
        // 若这里传读池，会出现「放行 100 个写、却只有 40 个连接可抢」的超时。
        let pool = self.write_pool.clone();
        let fut = f(pool);

        // 使用 tokio::spawn 执行异步操作
        let result = tokio::spawn(fut)
            .await
            .map_err(|e| format!("写任务崩溃: {}", e))?;

        drop(permit); // 任务完成，归还 permit
        result
    }
}

/// 初始化数据库
pub async fn init_db() -> Result<DbPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "mysql://chatuser:CHANGE_ME@127.0.0.1:3306/chat_server".to_string());

    // 识别数据库类型
    let db_type = DatabaseType::from_url(&database_url)
        .ok_or_else(|| format!("不支持的数据库 URL 格式: {}", mask_url(&database_url)))?;

    tracing::info!("📂 数据库类型: {}", db_type.as_str());
    tracing::info!("📂 数据库地址: {}", mask_url(&database_url));

    // 处理特殊 URL（金仓兼容 PostgreSQL 协议）
    let connect_url = match db_type {
        DatabaseType::Kingbase => database_url
            .replace("kingbase://", "postgres://")
            .replace("kb://", "postgres://"),
        _ => database_url.clone(),
    };

    // 创建连接池
    let pool = AnyPoolOptions::new()
        .max_connections(20)
        .min_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&connect_url)
        .await?;

    // 数据库特定初始化
    match db_type {
        DatabaseType::SQLite => {
            sqlx::query("PRAGMA foreign_keys = ON")
                .execute(&pool)
                .await?;
            sqlx::query("PRAGMA journal_mode = WAL")
                .execute(&pool)
                .await?;
        }
        DatabaseType::MySQL => {
            sqlx::query("SET NAMES utf8mb4")
                .execute(&pool)
                .await?;
        }
        _ => {}
    }

    // 运行迁移
    run_migrations(&pool, db_type).await?;

    let dialect = create_dialect(db_type);
    tracing::info!("✅ 数据库表结构初始化完成");

    // 创建占位的读池（与主池共享）
    let read_pool = pool.clone();
    // 写连接池（与主池共享）
    let write_pool = pool.clone();
    // 写并发控制信号量（大小=连接池上限，防止超载）
    let write_sem_size: usize = 80; // 80 permits，匹配 write_pool_max=80
    let write_sem = Arc::new(Semaphore::new(write_sem_size));
    tracing::info!("📊 写并发控制信号量: {} 个permit", write_sem_size);

    Ok(DbPool {
        pool,
        db_type,
        dialect: Arc::from(dialect),
        read_pool,
        write_pool,
        write_sem,
        write_url: connect_url,
        read_pool_max: 20,
        write_sem_size: 25,
    })
}

/// 运行数据库迁移
async fn run_migrations(pool: &sqlx::AnyPool, db_type: DatabaseType) -> Result<(), sqlx::Error> {
    match db_type {
        DatabaseType::MySQL => run_mysql_migrations(pool).await,
        DatabaseType::PostgreSQL => run_postgres_migrations(pool).await,
        DatabaseType::SQLite => run_sqlite_migrations(pool).await,
        DatabaseType::SQLServer => run_mssql_migrations(pool).await,
        DatabaseType::Dameng => run_dameng_migrations(pool).await,
        DatabaseType::Kingbase => run_kingbase_migrations(pool).await,
    }
}

// ========== MySQL 迁移 ==========
async fn run_mysql_migrations(pool: &sqlx::AnyPool) -> Result<(), sqlx::Error> {
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS users (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            username VARCHAR(64) NOT NULL UNIQUE,
            password_hash VARCHAR(255) NOT NULL,
            nickname VARCHAR(64) NOT NULL,
            avatar_url VARCHAR(512),
            status VARCHAR(16) NOT NULL DEFAULT 'offline',
            created_at VARCHAR(32) NOT NULL,
            updated_at VARCHAR(32) NOT NULL,
            INDEX idx_username (username),
            INDEX idx_status (status)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS friends (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            user_id BIGINT NOT NULL,
            friend_id BIGINT NOT NULL,
            status VARCHAR(16) NOT NULL DEFAULT 'pending',
            created_at VARCHAR(32) NOT NULL,
            UNIQUE KEY uk_user_friend (user_id, friend_id),
            INDEX idx_user_id (user_id),
            INDEX idx_friend_id (friend_id),
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
            FOREIGN KEY (friend_id) REFERENCES users(id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS friend_requests (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            from_user_id BIGINT NOT NULL,
            to_user_id BIGINT NOT NULL,
            status VARCHAR(16) NOT NULL DEFAULT 'pending',
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            INDEX idx_from_user (from_user_id),
            INDEX idx_to_user (to_user_id),
            FOREIGN KEY (from_user_id) REFERENCES users(id) ON DELETE CASCADE,
            FOREIGN KEY (to_user_id) REFERENCES users(id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS messages (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            sender_id BIGINT NOT NULL,
            receiver_id BIGINT,
            group_id BIGINT,
            content TEXT NOT NULL,
            msg_type VARCHAR(16) NOT NULL DEFAULT 'text',
            is_read INT NOT NULL DEFAULT 0,
            created_at VARCHAR(32) NOT NULL,
            INDEX idx_sender (sender_id),
            INDEX idx_receiver (receiver_id),
            INDEX idx_group (group_id),
            INDEX idx_created (created_at),
            INDEX idx_sender_created (sender_id, created_at),
            INDEX idx_receiver_created (receiver_id, created_at),
            FOREIGN KEY (sender_id) REFERENCES users(id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS `groups` (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            name VARCHAR(128) NOT NULL,
            owner_id BIGINT NOT NULL,
            avatar_url VARCHAR(512),
            description TEXT,
            created_at VARCHAR(32) NOT NULL,
            INDEX idx_owner (owner_id),
            FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS group_members (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            group_id BIGINT NOT NULL,
            user_id BIGINT NOT NULL,
            role VARCHAR(16) NOT NULL DEFAULT 'member',
            joined_at VARCHAR(32) NOT NULL,
            UNIQUE KEY uk_group_user (group_id, user_id),
            INDEX idx_group_id (group_id),
            INDEX idx_user_id (user_id),
            FOREIGN KEY (group_id) REFERENCES `groups`(id) ON DELETE CASCADE,
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS refresh_tokens (
            id BIGINT PRIMARY KEY AUTO_INCREMENT,
            user_id BIGINT NOT NULL,
            token VARCHAR(512) NOT NULL UNIQUE,
            expires_at VARCHAR(32) NOT NULL,
            created_at VARCHAR(32) NOT NULL,
            INDEX idx_user_id (user_id),
            INDEX idx_token (token(64)),
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
    "#).execute(pool).await?;

    Ok(())
}

// ========== PostgreSQL 迁移 ==========
async fn run_postgres_migrations(pool: &sqlx::AnyPool) -> Result<(), sqlx::Error> {
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS users (
            id BIGSERIAL PRIMARY KEY,
            username VARCHAR(64) NOT NULL UNIQUE,
            password_hash VARCHAR(255) NOT NULL,
            nickname VARCHAR(64) NOT NULL,
            avatar_url VARCHAR(512),
            status VARCHAR(16) NOT NULL DEFAULT 'offline',
            created_at VARCHAR(32) NOT NULL,
            updated_at VARCHAR(32) NOT NULL
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS friends (
            id BIGSERIAL PRIMARY KEY,
            user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            friend_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            status VARCHAR(16) NOT NULL DEFAULT 'pending',
            created_at VARCHAR(32) NOT NULL,
            UNIQUE(user_id, friend_id)
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS messages (
            id BIGSERIAL PRIMARY KEY,
            sender_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            receiver_id BIGINT,
            group_id BIGINT,
            content TEXT NOT NULL,
            msg_type VARCHAR(16) NOT NULL DEFAULT 'text',
            is_read BOOLEAN NOT NULL DEFAULT FALSE,
            created_at VARCHAR(32) NOT NULL
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS groups (
            id BIGSERIAL PRIMARY KEY,
            name VARCHAR(128) NOT NULL,
            owner_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            avatar_url VARCHAR(512),
            description TEXT,
            created_at VARCHAR(32) NOT NULL
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS group_members (
            id BIGSERIAL PRIMARY KEY,
            group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
            user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            role VARCHAR(16) NOT NULL DEFAULT 'member',
            joined_at VARCHAR(32) NOT NULL,
            UNIQUE(group_id, user_id)
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS friend_requests (
            id BIGSERIAL PRIMARY KEY,
            from_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            to_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            status VARCHAR(16) NOT NULL DEFAULT 'pending',
            created_at TIMESTAMP NOT NULL DEFAULT NOW()
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS refresh_tokens (
            id BIGSERIAL PRIMARY KEY,
            user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token VARCHAR(512) NOT NULL UNIQUE,
            expires_at VARCHAR(32) NOT NULL,
            created_at VARCHAR(32) NOT NULL
        )
    "#).execute(pool).await?;

    // 索引
    for sql in &[
        "CREATE INDEX IF NOT EXISTS idx_users_username ON users(username)",
        "CREATE INDEX IF NOT EXISTS idx_messages_sender ON messages(sender_id)",
        "CREATE INDEX IF NOT EXISTS idx_messages_receiver ON messages(receiver_id)",
        "CREATE INDEX IF NOT EXISTS idx_group_members_group ON group_members(group_id)",
    ] {
        sqlx::query(sql).execute(pool).await?;
    }

    Ok(())
}

// ========== SQLite 迁移 ==========
async fn run_sqlite_migrations(pool: &sqlx::AnyPool) -> Result<(), sqlx::Error> {
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            nickname TEXT NOT NULL,
            avatar_url TEXT,
            status TEXT NOT NULL DEFAULT 'offline',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS friends (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            friend_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL,
            UNIQUE(user_id, friend_id)
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            sender_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            receiver_id INTEGER,
            group_id INTEGER,
            content TEXT NOT NULL,
            msg_type TEXT NOT NULL DEFAULT 'text',
            is_read INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            owner_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            avatar_url TEXT,
            description TEXT,
            created_at TEXT NOT NULL
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS group_members (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            role TEXT NOT NULL DEFAULT 'member',
            joined_at TEXT NOT NULL,
            UNIQUE(group_id, user_id)
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS friend_requests (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            from_user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            to_user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TEXT NOT NULL
        )
    "#).execute(pool).await?;

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS refresh_tokens (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token TEXT NOT NULL UNIQUE,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL
        )
    "#).execute(pool).await?;

    for sql in &[
        "CREATE INDEX IF NOT EXISTS idx_users_username ON users(username)",
        "CREATE INDEX IF NOT EXISTS idx_messages_sender ON messages(sender_id)",
        "CREATE INDEX IF NOT EXISTS idx_messages_receiver ON messages(receiver_id)",
    ] {
        sqlx::query(sql).execute(pool).await?;
    }

    Ok(())
}

// ========== SQL Server 迁移 ==========
async fn run_mssql_migrations(pool: &sqlx::AnyPool) -> Result<(), sqlx::Error> {
    // SQL Server 使用 IF NOT EXISTS 模式
    let tables = vec![
        r#"IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='users' AND xtype='U')
        CREATE TABLE users (
            id BIGINT IDENTITY(1,1) PRIMARY KEY,
            username NVARCHAR(64) NOT NULL UNIQUE,
            password_hash NVARCHAR(255) NOT NULL,
            nickname NVARCHAR(64) NOT NULL,
            avatar_url NVARCHAR(512),
            status NVARCHAR(16) NOT NULL DEFAULT 'offline',
            created_at NVARCHAR(32) NOT NULL,
            updated_at NVARCHAR(32) NOT NULL
        )"#,
        r#"IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='friends' AND xtype='U')
        CREATE TABLE friends (
            id BIGINT IDENTITY(1,1) PRIMARY KEY,
            user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            friend_id BIGINT NOT NULL REFERENCES users(id),
            status NVARCHAR(16) NOT NULL DEFAULT 'pending',
            created_at NVARCHAR(32) NOT NULL
        )"#,
        r#"IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='messages' AND xtype='U')
        CREATE TABLE messages (
            id BIGINT IDENTITY(1,1) PRIMARY KEY,
            sender_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            receiver_id BIGINT,
            group_id BIGINT,
            content NVARCHAR(MAX) NOT NULL,
            msg_type NVARCHAR(16) NOT NULL DEFAULT 'text',
            is_read BIT NOT NULL DEFAULT 0,
            created_at NVARCHAR(32) NOT NULL
        )"#,
        r#"IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='groups' AND xtype='U')
        CREATE TABLE groups (
            id BIGINT IDENTITY(1,1) PRIMARY KEY,
            name NVARCHAR(128) NOT NULL,
            owner_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            avatar_url NVARCHAR(512),
            description NVARCHAR(MAX),
            created_at NVARCHAR(32) NOT NULL
        )"#,
        r#"IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='group_members' AND xtype='U')
        CREATE TABLE group_members (
            id BIGINT IDENTITY(1,1) PRIMARY KEY,
            group_id BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
            user_id BIGINT NOT NULL REFERENCES users(id),
            role NVARCHAR(16) NOT NULL DEFAULT 'member',
            joined_at NVARCHAR(32) NOT NULL
        )"#,
        r#"IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='friend_requests' AND xtype='U')
        CREATE TABLE friend_requests (
            id BIGINT IDENTITY(1,1) PRIMARY KEY,
            from_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            to_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            status NVARCHAR(16) NOT NULL DEFAULT 'pending',
            created_at DATETIME NOT NULL DEFAULT GETDATE()
        )"#,
        r#"IF NOT EXISTS (SELECT * FROM sysobjects WHERE name='refresh_tokens' AND xtype='U')
        CREATE TABLE refresh_tokens (
            id BIGINT IDENTITY(1,1) PRIMARY KEY,
            user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            token NVARCHAR(512) NOT NULL UNIQUE,
            expires_at NVARCHAR(32) NOT NULL,
            created_at NVARCHAR(32) NOT NULL
        )"#,
    ];

    for sql in tables {
        sqlx::query(sql).execute(pool).await?;
    }
    Ok(())
}

// ========== 达梦数据库迁移 ==========
async fn run_dameng_migrations(pool: &sqlx::AnyPool) -> Result<(), sqlx::Error> {
    // 达梦兼容 Oracle 语法，使用 EXECUTE IMMEDIATE 处理 IF NOT EXISTS
    let tables = vec![
        r#"CREATE TABLE IF NOT EXISTS users (
            id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            username VARCHAR(64) NOT NULL UNIQUE,
            password_hash VARCHAR(255) NOT NULL,
            nickname VARCHAR(64) NOT NULL,
            avatar_url VARCHAR(512),
            status VARCHAR(16) DEFAULT 'offline',
            created_at VARCHAR(32) NOT NULL,
            updated_at VARCHAR(32) NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS friends (
            id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            user_id BIGINT NOT NULL,
            friend_id BIGINT NOT NULL,
            status VARCHAR(16) DEFAULT 'pending',
            created_at VARCHAR(32) NOT NULL,
            UNIQUE(user_id, friend_id)
        )"#,
        r#"CREATE TABLE IF NOT EXISTS messages (
            id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            sender_id BIGINT NOT NULL,
            receiver_id BIGINT,
            group_id BIGINT,
            content CLOB NOT NULL,
            msg_type VARCHAR(16) DEFAULT 'text',
            is_read NUMBER(1) DEFAULT 0,
            created_at VARCHAR(32) NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS groups (
            id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            name VARCHAR(128) NOT NULL,
            owner_id BIGINT NOT NULL,
            avatar_url VARCHAR(512),
            description CLOB,
            created_at VARCHAR(32) NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS group_members (
            id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            group_id BIGINT NOT NULL,
            user_id BIGINT NOT NULL,
            role VARCHAR(16) DEFAULT 'member',
            joined_at VARCHAR(32) NOT NULL,
            UNIQUE(group_id, user_id)
        )"#,
        r#"CREATE TABLE IF NOT EXISTS friend_requests (
            id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            from_user_id BIGINT NOT NULL,
            to_user_id BIGINT NOT NULL,
            status VARCHAR(16) DEFAULT 'pending',
            created_at TIMESTAMP DEFAULT SYSDATE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS refresh_tokens (
            id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
            user_id BIGINT NOT NULL,
            token VARCHAR(512) NOT NULL UNIQUE,
            expires_at VARCHAR(32) NOT NULL,
            created_at VARCHAR(32) NOT NULL
        )"#,
    ];

    for sql in tables {
        sqlx::query(sql).execute(pool).await?;
    }
    Ok(())
}

// ========== 人大金仓迁移 ==========
async fn run_kingbase_migrations(pool: &sqlx::AnyPool) -> Result<(), sqlx::Error> {
    // 金仓兼容 PostgreSQL，使用相同迁移
    run_postgres_migrations(pool).await
}

/// 隐藏密码
fn mask_url(url: &str) -> String {
    if let Some(at) = url.rfind('@') {
        if let Some(slash) = url[..at].rfind("//") {
            let prefix = &url[..slash + 2];
            let suffix = &url[at..];
            return format!("{}***{}", prefix, suffix);
        }
    }
    url.to_string()
}

// ========== 读写分离三层架构 ==========

/// 写任务类型
pub async fn init_db_with_config(config: &AppConfig) -> Result<DbPool, Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "mysql://chatuser:CHANGE_ME@127.0.0.1:3306/chat_server".to_string());

    // 识别数据库类型
    let db_type = DatabaseType::from_url(&database_url)
        .ok_or_else(|| format!("不支持的数据库 URL 格式: {}", mask_url(&database_url)))?;

    tracing::info!("📂 数据库类型: {}", db_type.as_str());
    tracing::info!("📂 数据库地址: {}", mask_url(&database_url));

    // 处理特殊 URL（金仓兼容 PostgreSQL 协议）
    let connect_url = match db_type {
        DatabaseType::Kingbase => database_url
            .replace("kingbase://", "postgres://")
            .replace("kb://", "postgres://"),
        _ => database_url.clone(),
    };

    // 读取配置参数
    let read_max_conn = config.database.read_pool_max;
    let write_max_conn = config.database.write_pool_max;
    let worker_count = config.database.write_worker_num.unwrap_or(10) as usize;

    // 读写池总上限：默认 800（按 MySQL max_connections=1000 的 80% 设置），
    // 可通过 config.yaml 的 database.max_total_connections 覆盖。
    // 高并发（10K）场景下需要更大的连接池来消化瞬时峰值。
    let max_total_conn = config.database.max_total_connections.unwrap_or(800).max(120);
    let (read_max_conn, write_max_conn) = if read_max_conn + write_max_conn > max_total_conn {
        let scaled_read = read_max_conn * max_total_conn / (read_max_conn + write_max_conn);
        let scaled_write = max_total_conn - scaled_read;
        tracing::warn!(
            "⚠️ 连接池配置总量 {} 超过安全上限 {}，按比例缩减为 读={} 写={}",
            read_max_conn + write_max_conn, max_total_conn, scaled_read, scaled_write
        );
        (scaled_read, scaled_write)
    } else {
        (read_max_conn, write_max_conn)
    };

    tracing::info!("📊 读连接池: {} 连接", read_max_conn);
    tracing::info!("📊 写连接池: {} 连接", write_max_conn);

    // 创建读连接池（SELECT 操作）
    let read_pool = AnyPoolOptions::new()
        .max_connections(read_max_conn)
        .min_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&connect_url)
        .await?;

    // 创建写连接池（INSERT/UPDATE/DELETE，与读池物理隔离）
    let write_pool = AnyPoolOptions::new()
        .max_connections(write_max_conn)
        .min_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&connect_url)
        .await?;

    // 兼容旧代码：pool 与 read_pool 共享同一底层池（sqlx Pool 内部是 Arc，
    // clone 不会新建连接），避免历史上「三个池 180 连接 > MySQL 151」的超卖。
    let pool = read_pool.clone();

    // 写并发控制信号量（容量与写连接池一致，避免连接池耗尽）
    let sem_size = write_max_conn;
    let write_sem = Arc::new(Semaphore::new(sem_size as usize));
    tracing::info!("📊 写并发控制信号量: {} 个permit", sem_size);

    // 创建 SQL 方言
    let dialect: Arc<dyn SqlDialect> = Arc::from(create_dialect(db_type));

    // 数据库特定初始化
    match db_type {
        DatabaseType::SQLite => {
            sqlx::query("PRAGMA foreign_keys = ON")
                .execute(&pool)
                .await?;
            sqlx::query("PRAGMA journal_mode = WAL")
                .execute(&pool)
                .await?;
        }
        DatabaseType::MySQL => {
            sqlx::query("SET NAMES utf8mb4")
                .execute(&pool)
                .await?;
        }
        _ => {}
    }

    // 运行迁移
    run_migrations(&pool, db_type).await?;

    tracing::info!("✅ 数据库三层架构初始化完成");

    let db_url = if connect_url.contains("localhost") || connect_url.contains("127.0.0.1") {
        connect_url.replace("localhost", "127.0.0.1")
    } else {
        connect_url.clone()
    };

    let db_pool = DbPool {
        pool,
        db_type,
        dialect,
        read_pool,
        write_pool,
        write_sem,
        write_url: db_url,
        read_pool_max: read_max_conn,
        write_sem_size: sem_size,
    };

    Ok(db_pool)
}
