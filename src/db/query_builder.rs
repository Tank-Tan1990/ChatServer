//! SQL 查询构建器
//! 基于 SqlDialect 生成类型安全的 SQL

use crate::db::driver::{SqlDialect, DatabaseType};

/// 查询构建器
pub struct QueryBuilder<'a> {
    dialect: &'a dyn SqlDialect,
    table: String,
    columns: Vec<String>,
    wheres: Vec<String>,
    order_by: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

impl<'a> QueryBuilder<'a> {
    pub fn new(dialect: &'a dyn SqlDialect, table: &str) -> Self {
        Self {
            dialect,
            table: dialect.quote_identifier(table),
            columns: vec!["*".to_string()],
            wheres: Vec::new(),
            order_by: None,
            limit: None,
            offset: None,
        }
    }

    /// 选择列
    pub fn select(mut self, columns: &[&str]) -> Self {
        self.columns = columns
            .iter()
            .map(|c| self.dialect.quote_identifier(c))
            .collect();
        self
    }

    /// WHERE 条件
    pub fn where_eq(mut self, column: &str, placeholder_index: usize) -> Self {
        let col = self.dialect.quote_identifier(column);
        let placeholder = self.dialect.placeholder(placeholder_index);
        self.wheres.push(format!("{} = {}", col, placeholder));
        self
    }

    /// WHERE IS NULL
    pub fn where_is_null(mut self, column: &str) -> Self {
        let col = self.dialect.quote_identifier(column);
        self.wheres.push(format!("{} IS NULL", col));
        self
    }

    /// ORDER BY
    pub fn order_by_desc(mut self, column: &str) -> Self {
        let col = self.dialect.quote_identifier(column);
        self.order_by = Some(format!("{} DESC", col));
        self
    }

    /// LIMIT
    pub fn limit(mut self, limit: i64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// OFFSET
    pub fn offset(mut self, offset: i64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// 构建 SELECT SQL
    pub fn build_select(self) -> String {
        let mut sql = format!(
            "SELECT {} FROM {}",
            self.columns.join(", "),
            self.table
        );

        if !self.wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.wheres.join(" AND "));
        }

        if let Some(order) = self.order_by {
            sql.push_str(" ORDER BY ");
            sql.push_str(&order);
        }

        if let (Some(limit), Some(offset)) = (self.limit, self.offset) {
            sql.push_str(" ");
            sql.push_str(&self.dialect.pagination(limit, offset));
        } else if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        sql
    }
}

/// INSERT 构建器
pub struct InsertBuilder<'a> {
    dialect: &'a dyn SqlDialect,
    table: String,
    columns: Vec<String>,
    placeholders: Vec<String>,
}

impl<'a> InsertBuilder<'a> {
    pub fn new(dialect: &'a dyn SqlDialect, table: &str) -> Self {
        Self {
            dialect,
            table: dialect.quote_identifier(table),
            columns: Vec::new(),
            placeholders: Vec::new(),
        }
    }

    pub fn column(mut self, name: &str, placeholder_index: usize) -> Self {
        self.columns.push(self.dialect.quote_identifier(name));
        self.placeholders.push(self.dialect.placeholder(placeholder_index));
        self
    }

    /// 构建 INSERT SQL（带返回 ID）
    pub fn build_insert(self) -> String {
        let returning = self.dialect.returning_id();
        format!(
            "INSERT INTO {} ({}) VALUES ({}) {}",
            self.table,
            self.columns.join(", "),
            self.placeholders.join(", "),
            returning
        ).trim().to_string()
    }

    /// 构建 INSERT IGNORE SQL
    pub fn build_insert_ignore(self) -> String {
        self.dialect.insert_ignore(
            &self.table,
            &self.columns.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            &format!("({})", self.placeholders.join(", ")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::driver::MySqlDialect;

    #[test]
    fn test_select_query() {
        let dialect = MySqlDialect;
        let sql = QueryBuilder::new(&dialect, "users")
            .select(&["id", "username", "nickname"])
            .where_eq("status", 0)
            .order_by_desc("created_at")
            .limit(10)
            .build_select();

        assert!(sql.contains("SELECT `id`, `username`, `nickname`"));
        assert!(sql.contains("FROM `users`"));
        assert!(sql.contains("WHERE `status` = ?"));
        assert!(sql.contains("ORDER BY `created_at` DESC"));
        assert!(sql.contains("LIMIT 10"));
    }

    #[test]
    fn test_insert_query() {
        let dialect = MySqlDialect;
        let sql = InsertBuilder::new(&dialect, "users")
            .column("username", 0)
            .column("password_hash", 1)
            .column("nickname", 2)
            .build_insert();

        assert!(sql.contains("INSERT INTO `users`"));
        assert!(sql.contains("(`username`, `password_hash`, `nickname`)"));
        assert!(sql.contains("VALUES (?, ?, ?)"));
    }
}
