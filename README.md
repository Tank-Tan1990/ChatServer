# ChatServer

> 一个高并发、可水平扩展的即时通讯（IM）服务端，使用 Rust + Axum 构建，设计目标为 **10,000 并发连接**稳定支撑。

[![Rust](https://img.shields.io/badge/rust-1.74%2B-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Axum](https://img.shields.io/badge/web-axum%200.7-9cf.svg)](https://github.com/tokio-rs/axum)
[![CI](https://github.com/Tank-Tan1990/ChatServer/actions/workflows/ci.yml/badge.svg)](https://github.com/Tank-Tan1990/ChatServer/actions)

---

## 一、项目简介

ChatServer 是一个仿微信架构的后端即时通讯服务，提供账号体系、好友关系、单聊 / 群聊、实时消息推送、搜索等完整能力。项目最初由 AI 编程工具 **QClaw** 完成基础设计与实现，随后由 **WorkBuddy** 接续完成高并发性能优化（连接池、异步批量写入、密码哈希调优等），是两个 AI 工具协作迭代的产物。

核心设计指标：

- **10,000 并发**注册 / 登录 / 消息链路全通过（实测 100% 成功率）
- 连接池零泄漏
- 单服务进程即可承载万级长连接

---

## 二、功能特性

| 模块 | 能力 |
|---|---|
| **账号与认证** | 注册、登录、刷新令牌、登出、JWT（Bearer / `?token=` 双模式）、密码 Argon2id 哈希（旧哈希透明迁移）、多端登录 |
| **好友关系** | 好友列表、发送 / 接受 / 拒绝 / 删除好友请求、双向关系一致性校验 |
| **消息系统** | 单聊 / 群聊消息、消息已读回执、未读统计、历史消息分页、离线消息（落库） |
| **群组系统** | 建群、群详情、群成员、加入 / 退出 / 踢人、群主解散、群聊广播 |
| **搜索** | 按用户名 / 昵称搜索用户、搜索群组 |
| **实时通信** | 基于 WebSocket 的全双工推送：在线状态、单聊 / 群聊实时消息、多端同步、协议层 Ping/Pong 保活 |
| **平台运维** | 健康检查 `/health`、运行指标 `/ws/stats`、配置化限流、结构化 JSON 日志、分布式 Snowflake ID |

> 完整功能设计清单（50 项，含优先级与状态）请见 [`docs/01_功能设计清单.md`](docs/01_功能设计清单.md)。

---

## 三、技术栈

| 分类 | 选型 |
|---|---|
| 语言 | Rust 2021 Edition |
| Web 框架 | Axum 0.7（含 WebSocket） |
| 异步运行时 | Tokio（full） |
| 数据库 | MySQL 8.4，通过 `sqlx 0.8`（`AnyPool` 抽象，实际以 MySQL 验证） |
| 缓存 / 队列 | Redis 0.24 |
| 认证 | JWT（jsonwebtoken），Argon2id 密码哈希 |
| 日志 | tracing / tracing-subscriber（JSON 格式） |
| 部署 | 单二进制可执行文件，无外部依赖 |

---

## 四、系统架构

```
                         ┌─────────────────────────────┐
        客户端 ──HTTP────▶│         Axum Router          │
        (REST API)        │  ┌───────────────────────┐  │
                         │  │  Auth / Friends /      │  │
        客户端 ──WS──────▶│  │  Messages / Groups /   │  │
        (实时推送)        │  │  Search / Monitor      │  │
                         │  └───────────────────────┘  │
                         │            │                │
                         │   ┌────────┴─────────┐      │
                         │   │    AppState       │      │
                         │   │  - AnyPool(读/写)  │      │
                         │   │  - Redis          │      │
                         │   │  - Snowflake       │      │
                         │   │  - AsyncWriter     │      │
                         │   └────────┬─────────┘      │
                         └────────────┼────────────────┘
                                      │
                      ┌───────────────┼───────────────┐
                      ▼               ▼               ▼
                 MySQL 8.4        Redis          本地内存队列
              (读池300/写池500)  (在线状态/队列)  (消息异步批量写)
```

**关键设计**

- **读写分离连接池**：读池 300、写池 500，总量上限 800（按 MySQL `max_connections` 的 80% 安全线动态调整），通过信号量钳制并发写入，杜绝连接池耗尽。
- **消息异步批量写入**：`MESSAGE_ASYNC=1` 时，消息先进入内存通道，由 3 个后台 worker 每 50ms / 500 条批量事务提交，将 N 次 fsync 压缩为 1 次，绕开 `innodb_flush_log_at_trc_commit=1` 的写入瓶颈。
- **Snowflake ID**：消息 ID 由服务预生成（41bit 时间戳 + 10bit worker + 12bit 序列），规避线上 `messages.id` 无自增列的问题。
- **WebSocket 多端推送**：`uid → {conn_id → Sender}` 映射支持同一用户多设备同时在线；单聊推送给接收者全部设备 + 发送者其他设备，群聊广播给全部在线成员。

---

## 五、目录结构

```
ChatServer/
├── src/                 # Rust 源码
│   ├── main.rs          # 入口、路由注册、状态初始化
│   ├── api/             # 各业务 handler（auth/friends/messages/groups/search/ws/monitor）
│   ├── db/              # 数据库连接池、多库驱动、SQL 迁移
│   ├── ws/              # WebSocket 协议与实时推送
│   ├── config/          # 配置加载
│   ├── state/           # 全局应用状态
│   ├── snowflake.rs     # 分布式 ID 生成
│   ├── message_writer.rs# 消息异步批量写入
│   ├── rate_limit/      # 限流
│   └── monitor/         # 运行指标
├── docs/                # 产品设计文档（5 篇）
├── tests/               # 集成测试与压测脚本（Python）
├── config.yaml          # 服务配置（已脱敏，密钥需自行填写）
├── .env.example         # 环境变量模板
├── Cargo.toml
└── LICENSE
```

---

## 六、快速开始

### 前置条件

- Rust 1.74+（建议最新 stable）
- MySQL 8.4（已建库 `chat_server`）
- Redis 7+

### 1. 准备数据库

```sql
CREATE DATABASE IF NOT EXISTS chat_server
  CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;
```

服务首次启动会自动执行内置迁移脚本（`src/db/mod.rs`），创建 `users / messages / groups / group_members / friends / friend_requests / refresh_tokens` 等表。**注意**：线上 `group_members` 使用联合主键、`messages.id` 无自增列，属已知 schema 约束，请勿依赖 `AUTO_INCREMENT`。

### 2. 配置

复制环境变量模板并填写真实值（**密钥务必替换**）：

```bash
cp .env.example .env
# 编辑 .env：DATABASE_URL / JWT_SECRET / JWT_REFRESH_SECRET / REDIS_URL
```

或直接编辑 `config.yaml`（已内置占位密钥，需替换为强随机值）：

```bash
openssl rand -base64 32   # 生成 JWT_SECRET
```

### 3. 编译与运行

```bash
# 开发模式
cargo run

# 生产 / 压测模式（启用异步批量写入、放开限流）
RATE_LIMIT_RPM=100000 MESSAGE_ASYNC=1 cargo run --release
```

服务默认监听 `0.0.0.0:8080`。

### 4. 健康检查

```bash
curl http://127.0.0.1:8080/health
# {"status":"ok","version":"0.1.0",...}
```

---

## 七、配置说明

| 配置项 | 说明 | 默认值 |
|---|---|---|
| `server.port` | 监听端口 | `8080` |
| `database.url` | MySQL 连接串（也可用 `DATABASE_URL` 环境变量覆盖） | — |
| `database.max_total_connections` | 读写池总量上限 | `800` |
| `database.read_pool_max` / `write_pool_max` | 读 / 写池上限 | `300` / `500` |
| `redis.url` | Redis 连接串 | `redis://127.0.0.1:6379` |
| `jwt.secret` / `jwt.refresh_secret` | JWT 签名密钥（建议用环境变量 `JWT_SECRET` / `JWT_REFRESH_SECRET` 覆盖） | 占位值（**必须改**） |
| `rate_limit.requests_per_minute` | 每分钟请求上限（`RATE_LIMIT_RPM` 环境变量优先） | `100000` |

**安全提示**：发布版本中的 JWT 密钥、数据库密码均为占位符，**部署前必须替换为强随机值**，切勿使用仓库中的默认值。

---

## 八、API 接口概览

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/auth/register` | 注册 |
| POST | `/api/auth/login` | 登录 |
| POST | `/api/auth/refresh` | 刷新令牌 |
| POST | `/api/auth/logout` | 登出 |
| GET | `/api/user/info` | 用户信息 |
| GET/POST/DELETE | `/api/friends*`, `/api/friends/requests*`, `/api/friends/:id` | 好友关系 |
| GET/POST | `/api/messages*`, `/api/messages/read`, `/api/messages/unread` | 消息 |
| GET/POST/DELETE | `/api/groups*`, `/api/groups/:id/members*` | 群组 |
| GET | `/api/search/users`, `/api/search/groups` | 搜索 |
| GET | `/ws` | WebSocket 实时通道 |
| GET | `/ws/stats` | WS 连接统计 |
| GET | `/health`, `/status` | 健康检查 |

认证支持 `Authorization: Bearer <token>` 与 `?token=<token>` 两种方式。

---

## 九、测试

`tests/` 目录提供 Python 集成测试与压测脚本（无需第三方库，使用标准库 + aiohttp）：

```bash
# 冒烟测试（46 项核心流程）
python tests/smoke_test.py http://127.0.0.1:8080

# WebSocket 端到端（11 项，支持自定义地址）
python tests/ws_test.py http://127.0.0.1:8080

# 10K 并发压测（注册+登录+消息）
python tests/bench_10k.py http://127.0.0.1:8080

# 10K 并发压测（仅登录+消息，预置账号）
python tests/bench_10k_login_msg.py http://127.0.0.1:8080
```

> 压测前建议放开限流：`export RATE_LIMIT_RPM=100000`，并启用 `MESSAGE_ASYNC=1`。

### 性能实测（10K 并发，注册 + 登录 + 消息全链路）

| 并发 | 注册 | 登录 | 发消息 | 连接池泄漏 |
|---|---|---|---|---|
| 500 | 100% | 100% | 100% | 无 |
| 1000 | 100% | 100% | 100% | 无 |
| 2000 | 100% | 100% | 100% | 无 |
| 5000 | 100% | 100% | 100% | 无 |
| 8000 | 100% | 100% | 100% | 无 |
| 10000 | 100% | 100% | 100% | 无 |

消息 10K 并发 QPS ≈ 3300，P99 ≈ 2.7s；压测后读池 `in_use=0`，零泄漏。

---

## 十、部署建议

- **单实例**：直接以 `cargo build --release` 产物 + `config.yaml` 运行即可支撑万级并发。
- **密钥管理**：生产环境统一通过环境变量（`JWT_SECRET` / `JWT_REFRESH_SECRET` / `DATABASE_URL`）注入，避免落盘配置文件。
- **HTTPS**：`config.yaml` 的 `https` 段支持证书启用（默认关闭）。
- **水平扩展**：当前为单进程架构；如需多实例，需引入共享会话存储（Redis 已部分支持）与负载均衡。

---

## 十一、开发历程

本项目由两个 AI 编程工具协作完成：

- **QClaw（2026-04）**：完成产品定义、核心功能实现、基础架构与早期（未成功的）10K 并发尝试。
- **WorkBuddy（2026-09 ~ 10）**：定位并修复连接池耗尽、SQLx 连接泄漏、`MySQL fsync`、Argon2id CPU 瓶颈、WS 实时推送失效等根因，最终达成 10K 并发目标，并完成本文档体系。

详细的开发时间线、双工具贡献边界与关键设计收敛结论见 [`docs/04_开发历程与双工具协作.md`](docs/04_开发历程与双工具协作.md)。

---

## 十二、许可证

[MIT](LICENSE) © 2026 ChatServer Contributors
