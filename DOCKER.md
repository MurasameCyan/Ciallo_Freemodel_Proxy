# Docker 部署指南

## 工作原理

`https://work.freemodel.dev` 不是可用普通 Bearer 请求直接调用的公开 OpenAI 端点。它要求官方 WorkBuddy 客户端，通过本机 CodeBuddy ACP gateway 完成请求。

Docker 镜像因此采用以下边界：

- 代理运行在容器中，提供 OpenAI 兼容 `/v1/*` 接口；
- 官方 WorkBuddy 客户端运行在宿主机，并保持正常登录；
- 容器通过 `WORKBUDDY_ACP_URL` 连接宿主机的本地 gateway；
- 容器和仓库都不保存、复制或伪造 WorkBuddy 私有认证。

实测 `key.txt` 中 `fe_...` 形式的 Freemodel key 可以访问 `api.freemodel.dev`，但不能作为 `CODEBUDDY_AUTH_TOKEN` 或 `CODEBUDDY_API_KEY` 登录通用 CodeBuddy CLI。直接把通用 CLI 指向 `work.freemodel.dev/v1` 会被上游拒绝并提示只能由 WorkBuddy client 使用。因此，外部官方 gateway 是可验证且合规的 Docker 方案。

## 前置条件

1. 宿主机已安装、登录并运行官方 WorkBuddy。
2. WorkBuddy ACP gateway 可通过宿主机 `http://127.0.0.1:44741` 访问。
3. 使用以下任一环境：
   - Docker Desktop 4.34+，并启用 **Settings → Resources → Network → Enable host networking**；
   - Linux Docker Engine 的 host network driver。

WorkBuddy 官方桌面端当前面向 Windows/macOS；Docker Desktop 的 host networking 用于让 Linux 容器访问宿主机只监听 loopback 的 gateway。

## 快速开始

```bash
# Git Bash / WSL / Linux / macOS
cp .env.example .env
# 编辑 .env，设置 WORKBUDDY_EXTERNAL_CWD
docker compose up -d
docker compose logs -f
```

```powershell
# Windows PowerShell
Copy-Item .env.example .env
# 编辑 .env，设置 WORKBUDDY_EXTERNAL_CWD，例如：
# S:/AIWorker/Freemodel_Proxy/Ciallo_Freemodel_Proxy/workspace
Docker Compose up -d
Docker Compose logs -f
```

Compose 已默认配置：

```yaml
network_mode: host
environment:
  PROXY_HOST: 127.0.0.1
  FREEMODEL_BASE_URL: https://work.freemodel.dev/v1
  FREEMODEL_TRANSPORT: workbuddy_acp
  WORKBUDDY_SIDECAR_MODE: external
  WORKBUDDY_ACP_URL: http://127.0.0.1:44741
  WORKBUDDY_EXTERNAL_CWD: 宿主机上的真实工作目录
```

host 网络模式下不使用 `ports:` 映射。Linux Docker Engine 使用 host network driver；Docker Desktop 4.34+ 则提供宿主机与 Linux 容器之间的 L4 TCP/UDP 可达性。两者都只解决网络连接，不会让宿主机 WorkBuddy 识别容器路径 `/workspace`，所以必须单独配置 `WORKBUDDY_EXTERNAL_CWD`。

## `docker run`

```bash
docker run -d --name freemodel-proxy \
  --network host \
  -e PROXY_HOST=127.0.0.1 \
  -e FREEMODEL_BASE_URL=https://work.freemodel.dev/v1 \
  -e FREEMODEL_TRANSPORT=workbuddy_acp \
  -e WORKBUDDY_SIDECAR_MODE=external \
  -e WORKBUDDY_ACP_URL=http://127.0.0.1:44741 \
  -e WORKBUDDY_EXTERNAL_CWD="$(pwd)/workspace" \
  -v freemodel-proxy-data:/data \
  -v "$(pwd)/workspace:/workspace" \
  ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest
```

镜像名必须全小写，这是 GHCR 的要求。

## 配置说明

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `FREEMODEL_BASE_URL` | `https://work.freemodel.dev/v1` | 逻辑 WorkBuddy 服务 |
| `FREEMODEL_TRANSPORT` | `workbuddy_acp` | 受保护端点要求 ACP |
| `WORKBUDDY_SIDECAR_MODE` | `external` | Docker 不启动通用 CLI，复用宿主机官方 gateway |
| `WORKBUDDY_ACP_URL` | `http://127.0.0.1:44741` | host network 中的宿主机 gateway |
| `WORKBUDDY_EXTERNAL_CWD` | 无，必须设置 | 宿主机 WorkBuddy 可访问的真实工作目录，不是 `/workspace` |
| `WORKBUDDY_ACP_PASSWORD` | 空 | gateway 启用 password 模式时设置 |
| `PROXY_HOST` | Compose 中为 `127.0.0.1` | 代理监听地址 |
| `PROXY_PORT` | `40589` | 代理监听端口 |
| `PROXY_API_KEY` | 空 | 代理自身 Bearer 鉴权 |
| `PROXY_DEFAULT_PROJECT` | `/workspace` | headerless 客户端的默认项目 |
| `PROXY_SESSION_STORE` | `/data/sessions.json` | 代理会话元数据 |
| `PROXY_RUNTIME_DIR` | `/data/runtime` | 运行时文件 |

`FREEMODEL_API_KEY` 仅用于显式切换到 `https://api.freemodel.dev/v1 + http` 的回退模式；它不参与默认 WorkBuddy ACP 认证。

## 如何确认 gateway 端口

项目默认值为 `44741`。如果 WorkBuddy 当前使用其他端口：

1. 查看 WorkBuddy/CodeBuddy gateway 的启动信息或本机会话注册文件；
2. 使用实际端口覆盖 `.env`：

```dotenv
WORKBUDDY_ACP_URL=http://127.0.0.1:实际端口
```

3. gateway 开启密码认证时，同时设置：

```dotenv
WORKBUDDY_ACP_PASSWORD=你的本地gateway密码
```

不要提交 `.env`；它已被 `.gitignore` 和 `.dockerignore` 排除。

## 验证部署

```bash
# 进程与配置存活检查（不探测外部 gateway）
curl -sS http://127.0.0.1:40589/health

# external 模式 gateway readiness；不可达时返回非 2xx
curl -sS http://127.0.0.1:40589/ready

# 模型列表
curl -sS http://127.0.0.1:40589/v1/models \
  -H "Authorization: Bearer local-proxy"

# 真实 ACP 对话
curl -sS http://127.0.0.1:40589/v1/chat/completions \
  -H "Authorization: Bearer local-proxy" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5.6-sol","messages":[{"role":"user","content":"只回复 OK"}]}'
```

如果设置了 `PROXY_API_KEY`，将上面的 `local-proxy` 换成它的真实值。

## 暴露到局域网

默认配置只监听 `127.0.0.1`。如确需开放给可信局域网：

```dotenv
PROXY_API_KEY=强随机值
```

并把 Compose 中 `PROXY_HOST` 改为 `0.0.0.0`。host networking 会让代理端口可从宿主机网络访问，必须同时配置防火墙。不要把无鉴权代理暴露到公网。

## direct HTTP 回退模式

如果只想使用计费/限额的 `api.freemodel.dev`，可在 `.env` 设置：

```dotenv
FREEMODEL_BASE_URL=https://api.freemodel.dev/v1
FREEMODEL_TRANSPORT=http
FREEMODEL_API_KEY=你的fe_key
```

该模式不使用 WorkBuddy gateway，也不具备用户所要求的 WorkBuddy 免费使用路径。

## 故障排查

### `WorkBuddy ACP connection failed`

- 确认 WorkBuddy 已登录且正在运行；
- 确认 Docker Desktop 已启用 host networking；
- 确认 `WORKBUDDY_ACP_URL` 端口正确；
- 在宿主机检查 `http://127.0.0.1:端口/api/v1/health`；
- 如果 gateway 要求密码，设置 `WORKBUDDY_ACP_PASSWORD`。

### `This API can only be used with the WorkBuddy client`

说明绕过了官方 gateway，正在尝试让通用 CLI 或普通 HTTP 客户端直接访问受保护服务。恢复 `WORKBUDDY_SIDECAR_MODE=external`，并连接已登录的官方 WorkBuddy gateway。

### 容器启动即退出

确认 `./workspace` 目录存在，并已设置非空 `WORKBUDDY_EXTERNAL_CWD`。程序会 canonicalize `PROXY_DEFAULT_PROJECT=/workspace`，目录不存在会直接拒绝启动；external 模式缺少宿主机工作目录也会在配置加载阶段拒绝启动。

### 管理 API 被拒绝

`/proxy/*` 只接受 loopback 请求。默认 host 网络配置符合要求；若在 bridge 网络或经反向代理访问，这些路由会被拒绝，这是安全设计。

## GitHub Actions 自动发布

推送 `beta` 后，workflow 构建 `linux/amd64` 并发布：

- `ghcr.io/murasamecyan/ciallo_freemodel_proxy:beta`
- `ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest`

workflow 使用 `${GITHUB_REPOSITORY,,}` 将仓库名转换为 GHCR 要求的小写。首次发布后需在 GitHub Package settings 将容器包设为 Public。

构建过程不读取 `scr/key.txt`，也没有密钥 build arg。`.dockerignore` 明确排除 `key.txt`、`.env`、`config.json` 和常见私钥格式。
