# 发布到 GitHub 操作步骤

本目录 `githubcode/` 已经是**干净、可发布的完整仓库**（已脱敏密钥、已排除 `target/`、`.workbuddy/`、`data/` 等大目录与敏感目录）。

本机环境未安装 `gh` 且未登录 GitHub，因此**仓库创建与推送需你在本机执行最后一步**。以下两种方式任选其一。

---

## 方式 A：使用 GitHub CLI（推荐）

### 1. 安装并登录 gh
```powershell
# 安装（如未安装）：从 https://cli.github.com/ 下载，或用 winget
winget install --id GitHub.cli

# 登录（按提示在浏览器授权）
gh auth login
```

### 2. 在本目录初始化并提交
```powershell
cd F:\Desktop\ChatServer\githubcode
git init -b main
git add .
git commit -m "Initial commit: ChatServer 0.1.0 - 高并发 IM 服务端"
```

### 3. 创建仓库并推送
```powershell
# 创建私有或公开仓库（-p 公开，--private 私有）
gh repo create ChatServer --public --source=. --remote=origin --push
```
> 仓库名可自定义；若改了名，后续命令里的 `ChatServer` 同步替换即可。

---

## 方式 B：使用 GitHub 网页 + git 命令行

### 1. 网页创建仓库
- 打开 https://github.com/new
- Repository name 填 `ChatServer`
- 选择 **Public** 或 **Private**
- **不要**勾选 "Add a README / .gitignore / LICENSE"（本目录已包含）
- 点击 **Create repository**

### 2. 本地关联并推送
```powershell
cd F:\Desktop\ChatServer\githubcode
git init -b main
git add .
git commit -m "Initial commit: ChatServer 0.1.0 - 高并发 IM 服务端"

git remote add origin https://github.com/<你的用户名>/ChatServer.git
git branch -M main
git push -u origin main
```
> 推送时若提示认证，使用 GitHub 账号 + **Personal Access Token**（Settings → Developer settings → Tokens），密码框粘贴 Token。

---

## 推送前自检清单

- [ ] `config.yaml` 中的 JWT 密钥、数据库密码为占位符（非真实值）
- [ ] 已确认 `target/`、`.workbuddy/`、`data/`、`.env` 未被纳入（`git status` 验证）
- [ ] README 中的项目链接（徽章里的 `your-org/ChatServer`）已替换为真实地址
- [ ] `Cargo.lock` 已提交（二进制项目需可复现构建）

---

## 安全提醒

- 发布版本不含任何真实密钥，但**部署前务必**将 `config.yaml` 或环境变量中的密钥替换为强随机值。
- 生成强密钥：`openssl rand -base64 32`
- 切勿把本地 `.env`、真实证书、数据库备份提交到仓库。
