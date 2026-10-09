# 贡献指南

感谢你关注 ChatServer！在提交代码前，请阅读以下约定。

## 开发环境

- Rust 1.74+（`rustup` 安装 stable）
- MySQL 8.4、Redis 7+
- 推荐编辑器：VS Code + `rust-analyzer`

## 分支模型

- `main`：稳定可发布分支
- 功能开发请基于 `main` 创建 `feature/xxx` 分支
- 修复请创建 `fix/xxx` 分支

## 提交规范

提交信息建议采用 `type(scope): subject` 格式：

- `feat`：新功能
- `fix`：缺陷修复
- `perf`：性能优化
- `docs`：文档
- `refactor`：重构
- `test`：测试

示例：`fix(ws): 修复多端推送时连接映射错误`

## 代码质量

提交前请确保：

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

## 安全红线

- **严禁**在代码中硬编码密钥、密码、Token。所有敏感值必须通过环境变量或 `config.yaml` 占位符 + 环境变量覆盖。
- 数据库密码、JWT 密钥等一律使用占位符提交，由使用者自行注入。
- 不要提交 `target/`、`data/`、`.env`、`.workbuddy/` 等目录。

## PR 流程

1. Fork 并创建分支
2. 编写代码与测试
3. 确保 CI（fmt / clippy / build / test）通过
4. 提交 PR，描述改动动机与验证方式

## 测试

集成测试位于 `tests/`，使用 Python + aiohttp：

```bash
python tests/smoke_test.py http://127.0.0.1:8080
python tests/bench_10k.py    http://127.0.0.1:8080
```
