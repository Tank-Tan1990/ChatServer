//! 测试模块
//! 单元测试和集成测试

#[cfg(test)]
mod tests {
    use crate::api::auth::{generate_token, decode_token, JWT_SECRET};
    use crate::error::AppError;
    use crate::utils::{validate_username, validate_password, mask_phone, mask_email};

    /// 测试 JWT Token 生成
    #[test]
    fn test_generate_token() {
        let user_id = 123i64;
        let token = generate_token(user_id).unwrap();
        assert!(!token.is_empty());
        
        // 解密验证
        let claims = decode_token(&token, JWT_SECRET).unwrap();
        assert_eq!(claims.sub, "123");
    }

    /// 测试用户名验证 - 合法用户名
    #[test]
    fn test_validate_username_valid() {
        assert!(validate_username("user123"));
        assert!(validate_username("test_user"));
        assert!(validate_username("abc"));
    }

    /// 测试用户名验证 - 非法用户名
    #[test]
    fn test_validate_username_invalid() {
        assert!(!validate_username("ab"));       // 太短
        assert!(!validate_username("a".repeat(21).as_str())); // 太长
        assert!(!validate_username("user-name")); // 包含非法字符
        assert!(!validate_username("user name")); // 包含空格
    }

    /// 测试密码验证
    #[test]
    fn test_validate_password() {
        assert!(validate_password("123456"));
        assert!(validate_password("password"));
        assert!(!validate_password("12345")); // 太短
    }

    /// 测试手机号脱敏
    #[test]
    fn test_mask_phone() {
        assert_eq!(mask_phone("13812345678"), "138****5678");
        assert_eq!(mask_phone("123456"), "123456"); // 短号码
    }

    /// 测试邮箱脱敏
    #[test]
    fn test_mask_email() {
        assert_eq!(mask_email("test@example.com"), "t**@example.com");
        assert_eq!(mask_email("ab@example.com"), "**@example.com");
        assert_eq!(mask_email("a@b.com"), "**@b.com");
    }
}

/// API 端点测试
#[cfg(test)]
mod api_tests {
    use serde_json::json;

    /// 测试注册请求 JSON 解析
    #[test]
    fn test_register_request_parse() {
        let json = json!({
            "username": "testuser",
            "password": "password123",
            "nickname": "测试用户"
        });
        
        let req: crate::models::RegisterRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.username, "testuser");
        assert_eq!(req.password, "password123");
        assert_eq!(req.nickname, "测试用户");
    }

    /// 测试登录请求 JSON 解析
    #[test]
    fn test_login_request_parse() {
        let json = json!({
            "username": "testuser",
            "password": "password123"
        });
        
        let req: crate::models::LoginRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.username, "testuser");
        assert_eq!(req.password, "password123");
    }

    /// 测试发送消息请求 JSON 解析
    #[test]
    fn test_send_message_request_parse() {
        let json = json!({
            "receiver_id": 2,
            "content": "你好",
            "msg_type": "text"
        });
        
        let req: crate::models::SendMessageRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.receiver_id, Some(2));
        assert_eq!(req.content, "你好");
    }

    /// 测试创建群组请求 JSON 解析
    #[test]
    fn test_create_group_request_parse() {
        let json = json!({
            "name": "测试群组",
            "avatar_url": "https://example.com/avatar.png"
        });
        
        let req: crate::models::CreateGroupRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, "测试群组");
        assert_eq!(req.avatar_url, Some("https://example.com/avatar.png".to_string()));
    }
}

/// 错误类型测试
#[cfg(test)]
mod error_tests {
    use crate::error::{AppError, AppResult};
    use axum::http::StatusCode;

    /// 测试错误转响应
    #[test]
    fn test_error_into_response() {
        let error = AppError::BadRequest("测试错误".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// 测试认证错误
    #[test]
    fn test_auth_error() {
        let error = AppError::Auth("未授权".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// 测试数据库错误
    #[test]
    fn test_database_error() {
        let error = AppError::Database(sqlx::Error::RowNotFound);
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

/// 分页测试
#[cfg(test)]
mod pagination_tests {
    use crate::pagination::{PageResponse, PaginationInfo};

    #[test]
    fn test_page_response() {
        let data = vec![1, 2, 3];
        let response = PageResponse::new(data.clone(), 10, 1, 3);
        
        assert_eq!(response.data.len(), 3);
        assert_eq!(response.pagination.page, 1);
        assert_eq!(response.pagination.page_size, 3);
        assert_eq!(response.pagination.total, 10);
        assert_eq!(response.pagination.total_pages, 4);
        assert!(response.pagination.has_next);
        assert!(!response.pagination.has_prev);
    }
}