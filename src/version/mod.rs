//! API 版本管理模块
//! 支持多版本 API 共存

use std::collections::HashMap;

/// API 版本配置
#[derive(Debug, Clone)]
pub struct ApiVersion {
    pub prefix: String,
    pub deprecated: bool,
    pub description: String,
}

/// API 路由工厂
pub struct ApiRouter {
    versions: HashMap<String, ApiVersion>,
}

impl ApiRouter {
    pub fn new() -> Self {
        let mut versions = HashMap::new();
        versions.insert(
            "v1".to_string(),
            ApiVersion {
                prefix: "/api/v1".to_string(),
                deprecated: false,
                description: "第一版 API".to_string(),
            },
        );
        Self { versions }
    }

    /// 获取版本信息
    pub fn get_version(&self, version: &str) -> Option<&ApiVersion> {
        self.versions.get(version)
    }

    /// 列出所有版本
    pub fn list_versions(&self) -> Vec<(&String, &ApiVersion)> {
        self.versions.iter().collect()
    }
}

impl Default for ApiRouter {
    fn default() -> Self {
        Self::new()
    }
}