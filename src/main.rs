//! ChatServer 主入口
//! 类微信后端服务器 - Rust 实现
//! 支持分布式部署（10000+ 并发）

mod api;
mod config;
mod db;
mod error;
mod message_writer;
mod middleware;
mod models;
mod monitor;
mod pagination;
mod rate_limit;
mod redis;
mod snowflake;
mod state;
mod utils;
mod version;
mod ws;

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use axum::routing::get;
use axum::middleware as axum_middleware;

#[tokio::main]
async fn main() {
    // 安装 sqlx Any 驱动（支持多数据库）
    sqlx::any::install_default_drivers();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "chat_server=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("🦞 ChatServer 启动中...");
    tracing::info!("📦 版本: {}", env!("CARGO_PKG_VERSION"));

    // 加载配置
    let app_config = config::load_config(None);
    tracing::info!("⚙️ 配置加载完成: {}:{}", 
        app_config.server.host, 
        app_config.server.port
    );

    // 初始化数据库（写操作走 Semaphore + spawn_blocking）
    let db_pool = db::init_db_with_config(&app_config).await.expect("数据库初始化失败");
    tracing::info!("✅ 数据库初始化成功");
    tracing::info!("   - 读连接池: max={}", app_config.database.read_pool_max);
    tracing::info!("   - 写并发控制: Semaphore({})", db_pool.write_sem_size);

    // 初始化 Redis（分布式支持）
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let redis_pool = match redis::create_redis_pool(&redis_url, 10).await {
        Ok(pool) => {
            tracing::info!("✅ Redis 连接池创建成功: {}", redis_url);
            pool
        }
        Err(e) => {
            tracing::warn!("⚠️ Redis 连接失败: {}，将使用单机模式", e);
            // 创建空池用于单机模式
            redis::create_redis_pool("redis://127.0.0.1:6379", 1).await.expect("Redis required")
        }
    };

    // 解析限流配置：env RATE_LIMIT_RPM 优先，config.yaml rate_limit 兜底，默认压测友好值
    let rate_limit_rpm: u32 = std::env::var("RATE_LIMIT_RPM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(app_config.rate_limit.requests_per_minute);
    tracing::info!("🚦 限流: {} 请求/分钟 (enabled={})", rate_limit_rpm, app_config.rate_limit.enabled);

    // 创建应用状态（分布式版本）
    let app_state = state::AppState::new(db_pool, redis_pool)
        .with_rate_limit(rate_limit_rpm);

    // CORS 配置
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // 构建路由
    let app = Router::new()
        // 健康检查 & 监控
        .route("/health", get(monitor::health_check))
        .route("/status", get(monitor::system_status))
        
        // API 路由
        .route("/api/auth/register", axum::routing::post(api::auth::register))
        .route("/api/auth/login", axum::routing::post(api::auth::login))
        .route("/api/auth/refresh", axum::routing::post(api::auth::refresh_token))
        .route("/api/auth/logout", axum::routing::post(api::auth::logout))
        .route("/api/user/info", axum::routing::get(api::auth::get_user_info))
        
        .route("/api/friends", axum::routing::get(api::friends::list_friends))
        .route("/api/friends", axum::routing::post(api::friends::add_friend))
        .route("/api/friends/requests", axum::routing::get(api::friends::list_friend_requests))
        .route("/api/friends/handle", axum::routing::post(api::friends::handle_friend_request))
        .route("/api/friends/:id", axum::routing::delete(api::friends::remove_friend))
        
        .route("/api/messages", axum::routing::get(api::messages::list_messages))
        .route("/api/messages", axum::routing::post(api::messages::send_message))
        .route("/api/messages/read", axum::routing::post(api::messages::mark_messages_read))
        .route("/api/messages/unread", axum::routing::get(api::messages::get_unread_count))
        
        .route("/api/groups", axum::routing::get(api::groups::list_groups))
        .route("/api/groups", axum::routing::post(api::groups::create_group))
        .route("/api/groups/:id", axum::routing::get(api::groups::get_group_info))
        .route("/api/groups/:id", axum::routing::delete(api::groups::disband_group))
        .route("/api/groups/:id/join", axum::routing::post(api::groups::join_group))
        .route("/api/groups/:id/members", axum::routing::get(api::groups::list_group_members))
        .route("/api/groups/:id/members", axum::routing::post(api::groups::add_group_member))
        .route("/api/groups/:id/members/:member_id", axum::routing::delete(api::groups::remove_group_member))
        .route("/api/groups/:id/leave", axum::routing::post(api::groups::leave_group))
        
        // 运维监控
        // 注意：api::monitor::circuit_status 是返回固定假数据的占位实现，不注册，
        // 等熔断器真正接入业务后再上，避免暴露误导性的监控端点
        .route("/api/ws/stats", axum::routing::get(api::ws::ws_stats))

        // Search
        .route("/api/search/users", axum::routing::get(api::search::search_users))
        .route("/api/search/groups", axum::routing::get(api::search::search_groups))
        
        // WebSocket
        .route("/ws", get(ws::ws_handler))
        
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(axum_middleware::from_fn_with_state(app_state.clone(), middleware::rate_limit_middleware))
        .with_state(app_state);

    // 启动服务器
    let addr = SocketAddr::from(([0, 0, 0, 0], app_config.server.port));
    tracing::info!("🚀 服务器运行在 http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("服务器启动失败");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("注册 Ctrl+C 信号处理器失败");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("注册 terminate 信号处理器失败")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("🛑 收到关闭信号，正在停止服务...");
}