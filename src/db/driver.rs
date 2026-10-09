//! 数据库驱动抽象层 - 简化版
//! 支持 MySQL, PostgreSQL, SQLite, SQL Server, 达梦, 人大金仓


/// 数据库类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseType {
    MySQL,
    PostgreSQL,
    SQLite,
    SQLServer,
    Dameng,      // 达梦数据库
    Kingbase,    // 人大金仓
}

impl DatabaseType {
    /// 从连接字符串识别数据库类型
    pub fn from_url(url: &str) -> Option<Self> {
        let url_lower = url.to_lowercase();
        if url_lower.starts_with("mysql://") {
            Some(Self::MySQL)
        } else if url_lower.starts_with("postgres://") || url_lower.starts_with("postgresql://") {
            Some(Self::PostgreSQL)
        } else if url_lower.starts_with("sqlite:") {
            Some(Self::SQLite)
        } else if url_lower.starts_with("mssql://") || url_lower.starts_with("sqlserver://") {
            Some(Self::SQLServer)
        } else if url_lower.starts_with("dm://") || url_lower.starts_with("dameng://") {
            Some(Self::Dameng)
        } else if url_lower.starts_with("kingbase://") || url_lower.starts_with("kb://") {
            Some(Self::Kingbase)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MySQL => "mysql",
            Self::PostgreSQL => "postgresql",
            Self::SQLite => "sqlite",
            Self::SQLServer => "mssql",
            Self::Dameng => "dameng",
            Self::Kingbase => "kingbase",
        }
    }
}

/// SQL 方言特性
pub trait SqlDialect: Send + Sync {
    /// 获取数据库类型
    fn db_type(&self) -> DatabaseType;
    
    /// 获取自增主键语法
    fn auto_increment(&self) -> &'static str;
    
    /// 获取布尔类型
    fn boolean_type(&self) -> &'static str;
    
    /// 获取文本类型
    fn text_type(&self) -> &'static str;
    
    /// 获取时间戳类型
    fn timestamp_type(&self) -> &'static str;
    
    /// 获取创建表的额外选项
    fn table_options(&self) -> &'static str;
    
    /// 获取分页语法
    fn pagination(&self, limit: i64, offset: i64) -> String;
    
    /// 获取当前时间函数
    fn now(&self) -> &'static str;
    
    /// 引号标识符
    fn quote_identifier(&self, name: &str) -> String;
    
    /// 参数占位符 (?, $1, :1 等)
    fn placeholder(&self, index: usize) -> String;
    
    /// 获取最后插入 ID 的 SQL
    fn last_insert_id(&self) -> &'static str;
    
    /// INSERT IGNORE / ON CONFLICT DO NOTHING
    fn insert_ignore(&self, table: &str, columns: &[&str], values: &str) -> String;
    
    /// RETURNING id 语法
    fn returning_id(&self) -> &'static str;
}

/// 创建方言实例
pub fn create_dialect(db_type: DatabaseType) -> Box<dyn SqlDialect> {
    match db_type {
        DatabaseType::MySQL => Box::new(MySqlDialect),
        DatabaseType::PostgreSQL => Box::new(PostgresDialect),
        DatabaseType::SQLite => Box::new(SQLiteDialect),
        DatabaseType::SQLServer => Box::new(SQLServerDialect),
        DatabaseType::Dameng => Box::new(DamengDialect),
        DatabaseType::Kingbase => Box::new(KingbaseDialect),
    }
}

// ========== MySQL 方言 ==========
pub struct MySqlDialect;

impl SqlDialect for MySqlDialect {
    fn db_type(&self) -> DatabaseType { DatabaseType::MySQL }
    fn auto_increment(&self) -> &'static str { "AUTO_INCREMENT" }
    fn boolean_type(&self) -> &'static str { "TINYINT(1)" }
    fn text_type(&self) -> &'static str { "TEXT" }
    fn timestamp_type(&self) -> &'static str { "VARCHAR(32)" }
    fn table_options(&self) -> &'static str { "ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci" }
    fn pagination(&self, limit: i64, offset: i64) -> String {
        format!("LIMIT {} OFFSET {}", limit, offset)
    }
    fn now(&self) -> &'static str { "NOW()" }
    fn quote_identifier(&self, name: &str) -> String {
        format!("`{}`", name.replace('`', "``"))
    }
    fn placeholder(&self, _index: usize) -> String { "?".to_string() }
    fn last_insert_id(&self) -> &'static str { "SELECT LAST_INSERT_ID()" }
    fn insert_ignore(&self, table: &str, columns: &[&str], values: &str) -> String {
        format!("INSERT IGNORE INTO {} ({}) VALUES {}", table, columns.join(", "), values)
    }
    fn returning_id(&self) -> &'static str { "" }
}

// ========== PostgreSQL 方言 ==========
pub struct PostgresDialect;

impl SqlDialect for PostgresDialect {
    fn db_type(&self) -> DatabaseType { DatabaseType::PostgreSQL }
    fn auto_increment(&self) -> &'static str { "SERIAL" }
    fn boolean_type(&self) -> &'static str { "BOOLEAN" }
    fn text_type(&self) -> &'static str { "TEXT" }
    fn timestamp_type(&self) -> &'static str { "TIMESTAMP" }
    fn table_options(&self) -> &'static str { "" }
    fn pagination(&self, limit: i64, offset: i64) -> String {
        format!("LIMIT {} OFFSET {}", limit, offset)
    }
    fn now(&self) -> &'static str { "NOW()" }
    fn quote_identifier(&self, name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
    fn placeholder(&self, index: usize) -> String { format!("${}", index + 1) }
    fn last_insert_id(&self) -> &'static str { "RETURNING id" }
    fn insert_ignore(&self, table: &str, columns: &[&str], values: &str) -> String {
        format!("INSERT INTO {} ({}) VALUES {} ON CONFLICT DO NOTHING", table, columns.join(", "), values)
    }
    fn returning_id(&self) -> &'static str { "RETURNING id" }
}

// ========== SQLite 方言 ==========
pub struct SQLiteDialect;

impl SqlDialect for SQLiteDialect {
    fn db_type(&self) -> DatabaseType { DatabaseType::SQLite }
    fn auto_increment(&self) -> &'static str { "INTEGER PRIMARY KEY AUTOINCREMENT" }
    fn boolean_type(&self) -> &'static str { "INTEGER" }
    fn text_type(&self) -> &'static str { "TEXT" }
    fn timestamp_type(&self) -> &'static str { "TEXT" }
    fn table_options(&self) -> &'static str { "" }
    fn pagination(&self, limit: i64, offset: i64) -> String {
        format!("LIMIT {} OFFSET {}", limit, offset)
    }
    fn now(&self) -> &'static str { "datetime('now')" }
    fn quote_identifier(&self, name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
    fn placeholder(&self, _index: usize) -> String { "?".to_string() }
    fn last_insert_id(&self) -> &'static str { "SELECT last_insert_rowid()" }
    fn insert_ignore(&self, table: &str, columns: &[&str], values: &str) -> String {
        format!("INSERT OR IGNORE INTO {} ({}) VALUES {}", table, columns.join(", "), values)
    }
    fn returning_id(&self) -> &'static str { "" }
}

// ========== SQL Server 方言 ==========
pub struct SQLServerDialect;

impl SqlDialect for SQLServerDialect {
    fn db_type(&self) -> DatabaseType { DatabaseType::SQLServer }
    fn auto_increment(&self) -> &'static str { "BIGINT IDENTITY(1,1) PRIMARY KEY" }
    fn boolean_type(&self) -> &'static str { "BIT" }
    fn text_type(&self) -> &'static str { "NVARCHAR(MAX)" }
    fn timestamp_type(&self) -> &'static str { "DATETIME2" }
    fn table_options(&self) -> &'static str { "" }
    fn pagination(&self, limit: i64, offset: i64) -> String {
        format!("OFFSET {} ROWS FETCH NEXT {} ROWS ONLY", offset, limit)
    }
    fn now(&self) -> &'static str { "GETDATE()" }
    fn quote_identifier(&self, name: &str) -> String {
        format!("[{}]", name.replace(']', "]]"))
    }
    fn placeholder(&self, index: usize) -> String { format!("@p{}", index + 1) }
    fn last_insert_id(&self) -> &'static str { "SELECT SCOPE_IDENTITY()" }
    fn insert_ignore(&self, table: &str, columns: &[&str], values: &str) -> String {
        format!("IF NOT EXISTS (SELECT 1 FROM {} WHERE ...) INSERT INTO {} ({}) VALUES {}", 
                table, table, columns.join(", "), values)
    }
    fn returning_id(&self) -> &'static str { "OUTPUT INSERTED.id" }
}

// ========== 达梦数据库方言 ==========
pub struct DamengDialect;

impl SqlDialect for DamengDialect {
    fn db_type(&self) -> DatabaseType { DatabaseType::Dameng }
    fn auto_increment(&self) -> &'static str { "BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY" }
    fn boolean_type(&self) -> &'static str { "NUMBER(1)" }
    fn text_type(&self) -> &'static str { "CLOB" }
    fn timestamp_type(&self) -> &'static str { "TIMESTAMP" }
    fn table_options(&self) -> &'static str { "" }
    fn pagination(&self, limit: i64, offset: i64) -> String {
        format!("LIMIT {} OFFSET {}", limit, offset)
    }
    fn now(&self) -> &'static str { "SYSTIMESTAMP" }
    fn quote_identifier(&self, name: &str) -> String {
        format!("\"{}\"", name.to_uppercase())
    }
    fn placeholder(&self, index: usize) -> String { format!(":{}", index + 1) }
    fn last_insert_id(&self) -> &'static str { "SELECT IDENTITY_VAL_LOCAL()" }
    fn insert_ignore(&self, table: &str, columns: &[&str], values: &str) -> String {
        format!("MERGE INTO {} USING (SELECT {} FROM DUAL) src ON (...) WHEN NOT MATCHED THEN INSERT ({}) VALUES {}",
                table, columns.join(", "), columns.join(", "), values)
    }
    fn returning_id(&self) -> &'static str { "RETURNING id INTO ?" }
}

// ========== 人大金仓方言 ==========
pub struct KingbaseDialect;

impl SqlDialect for KingbaseDialect {
    fn db_type(&self) -> DatabaseType { DatabaseType::Kingbase }
    fn auto_increment(&self) -> &'static str { "SERIAL" }
    fn boolean_type(&self) -> &'static str { "BOOLEAN" }
    fn text_type(&self) -> &'static str { "TEXT" }
    fn timestamp_type(&self) -> &'static str { "TIMESTAMP" }
    fn table_options(&self) -> &'static str { "" }
    fn pagination(&self, limit: i64, offset: i64) -> String {
        format!("LIMIT {} OFFSET {}", limit, offset)
    }
    fn now(&self) -> &'static str { "NOW()" }
    fn quote_identifier(&self, name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
    fn placeholder(&self, index: usize) -> String { format!("${}", index + 1) }
    fn last_insert_id(&self) -> &'static str { "RETURNING id" }
    fn insert_ignore(&self, table: &str, columns: &[&str], values: &str) -> String {
        format!("INSERT INTO {} ({}) VALUES {} ON CONFLICT DO NOTHING", table, columns.join(", "), values)
    }
    fn returning_id(&self) -> &'static str { "RETURNING id" }
}
