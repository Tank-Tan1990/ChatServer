# 安全政策

## 支持的版本

| 版本 | 状态 |
|---|---|
| 0.1.x | ✅ 维护中 |

## 报告漏洞

如果你发现安全漏洞，**请勿通过公开 Issue 披露**。请通过以下方式私报告知：

- 邮件：`security@example.com`（请替换为实际安全联系人）
- 或在 GitHub 上创建 **Private Security Advisory**

请在报告中包含：

1. 漏洞类型与影响范围
2. 复现步骤
3. 建议的修复方案（如有）

我们将在 **72 小时内**确认收到，并尽快评估与修复。

## 密钥与配置安全

- 本项目**不**在仓库中存放任何真实密钥。
- `config.yaml` 与 `.env.example` 中的 JWT 密钥、数据库密码均为占位符。
- 部署前必须使用强随机值替换，并优先通过环境变量（`JWT_SECRET` / `JWT_REFRESH_SECRET` / `DATABASE_URL`）注入。
- 生成强密钥：

```bash
openssl rand -base64 32
```

## 已知安全设计

- 密码使用 **Argon2id** 哈希存储，旧版 bcrypt / SHA256 哈希在登录成功后透明迁移为 Argon2id。
- JWT 采用 `HS256` 签名，访问令牌默认有效期 120 分钟，刷新令牌 7 天。
- 默认 `ARGON2_MODE=secure` 可切换为高强度参数（`m=19MiB, t=2, p=1`）。
