//! 分页模块
//! 通用分页响应结构

use serde::{Deserialize, Serialize};

/// 分页请求参数
#[derive(Debug, Deserialize)]
pub struct PageParams {
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

impl Default for PageParams {
    fn default() -> Self {
        Self {
            page: Some(1),
            page_size: Some(20),
        }
    }
}

/// 分页响应结构
#[derive(Debug, Serialize)]
pub struct PageResponse<T> {
    pub data: Vec<T>,
    pub pagination: PaginationInfo,
}

/// 分页信息
#[derive(Debug, Serialize)]
pub struct PaginationInfo {
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub total_pages: usize,
    pub has_next: bool,
    pub has_prev: bool,
}

impl<T> PageResponse<T> {
    pub fn new(data: Vec<T>, total: usize, page: usize, page_size: usize) -> Self {
        let total_pages = (total + page_size - 1) / page_size;
        Self {
            data,
            pagination: PaginationInfo {
                page,
                page_size,
                total,
                total_pages,
                has_next: page < total_pages,
                has_prev: page > 1,
            }
        }
    }
}

/// 分页宏 - 用于快速实现分页查询
#[macro_export]
macro_rules! paginate {
    ($query:expr, $page:expr, $page_size:expr) => {{
        let offset = ($page - 1) * $page_size;
        $query
            .bind($page_size as i64)
            .bind(offset as i64)
            .fetch_all(&state.db)
            .await
    }};
}