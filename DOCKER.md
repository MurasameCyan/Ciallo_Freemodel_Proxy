# Docker 部署指南

## 推荐路线：cc.freemodel.dev，只要一个 key

`https://cc.freemodel.dev/v1` 讲 Anthropic Messages 协议，鉴权只看 `Bearer {FREEMODEL_API_KEY}`，**不需要 CodeBuddy 登录，也不需要在宿主机装任何东西**。代理负责把客户端的 OpenAI Chat Completions 协议翻译成 Anthropic Messages 再翻译回来，因此现有客户端不用改。

这是 `docker-compose.yml` 和 `.env.example` 的默认配置：

```bash
cp .env.example .env
# 编辑 .env，填入 FREEMODEL_API_KEY，并设一个 PROXY_API_KEY
docker compose up -d
curl -H "Authorization: Bearer $PROXY_API_KEY" http://127.0.0.1:40589/v1/models
```

`/health` 会显示 `"transport":"cc_anthropic"` 与 `"upstream_mode":"direct_anthropic"`。

也可以先不填 key，启动后再写入（存到 `/data` 命名卷，容器重建不丢）：

```bash
docker exec -it freemodel-proxy freemodel-workbuddy-proxy key set
docker compose restart
```

可用模型只有三个真实后端：`claude-opus-5`、`claude-fable-5`、`claude-haiku-4-5-20251001`，`/v1/models` 在这个 transport 下只报这三个。其它 Claude 别名（如 `claude-sonnet-5`）会被上游静默改换成别的后端，代理按 `message_start.model` 把真实后端名回显给客户端，因此响应里的 `model` 可能和你请求的不一样，这是上游行为而不是代理改写。非 `claude-*` 的名字（如客户端默认的 `gpt-4o`）上游不认，代理会直接用 `claude-opus-5` 起步。

上游是共享容器池，占满时返回 500 `Maximum number of running container instances exceeded`，与账号配额无关且随机发生。代理遇到这个错误会沿 `claude-opus-5 → claude-fable-5 → claude-haiku-4-5-20251001` 换后端重试，只有全部失败才回 502。4xx（含 401 key 无效）不重试。

每次响应都带 `usage`：`prompt_tokens` 含缓存读取，`prompt_tokens_details.cached_tokens` 单列，流式在 finish chunk 上给。用它核对实际消耗，不要依赖官方后台的用量展示。

## CodeBuddy P0 免宿主安装实验

`:codebuddy-p0` 是独立实验标签，在同一 Linux 容器内运行官方 CodeBuddy Code 2.133.0 gateway 和 Rust 代理。它只验证纯 Docker 官方登录是否成立，不会覆盖 `:beta` 或 `:latest`；稳定版继续使用宿主机已登录的 external gateway。

```bash
docker pull ghcr.io/murasamecyan/ciallo_freemodel_proxy:codebuddy-p0
docker compose -f docker-compose.codebuddy-p0.yml up -d
docker exec -it ciallo-codebuddy-p0 codebuddy
```

在容器内官方 CLI 输入 `/login`，按官方流程在浏览器完成登录。不要设置 `CODEBUDDY_API_KEY=fe_...` 或 `CODEBUDDY_AUTH_TOKEN=fe_...`：Freemodel key 不是 CodeBuddy 登录凭据。

登录后执行一条真实 ACP 请求：

```bash
RUN_LIVE_ACP_TEST=1 bash test/test-codebuddy-p0.sh
```

然后重启并重复请求，确认登录态确实保存在命名卷的 `/data/codebuddy-home`：

```bash
docker compose -f docker-compose.codebuddy-p0.yml restart
RUN_LIVE_ACP_TEST=1 bash test/test-codebuddy-p0.sh
```

只有官方登录、真实 ACP 响应和重启后再次响应三项都通过，P0 才算验收成功。`docker compose -f docker-compose.codebuddy-p0.yml down -v` 会删除 `/data` volume 和登录态，不要把它当作普通停止命令。

| 现象 | 含义 | 操作 |
| --- | --- | --- |
| 容器启动即退出，日志有 `/workspace 不可写` | 宿主机 `./workspace` 属主与容器内 uid 10001 不匹配 | 按提示执行 `mkdir -p ./workspace && sudo chown 10001:10001 ./workspace`，再 `docker compose -f docker-compose.codebuddy-p0.yml up -d` |
| `44741` health 不通 | gateway 未启动 | 在本机检查 `/data/runtime/codebuddy.log`；日志可能含敏感上下文，不得公开整份内容 |
| 模型请求返回 401/403 | gateway 未登录或账号无权 | 运行官方 `/login`；不要设置 `CODEBUDDY_API_KEY=fe_...` |
| 登录回调打不开 | P0 登录链路不成立 | 记录浏览器 URL 和错误码，但删除 token；停止 P1 |
| 重启后再次要求登录 | 登录态未正确落在 `/data` | 不发布 P1，先定位官方实际配置目录 |
| 返回 quota、capacity 或 `max_instances` | 官方账号限制 | 等待或清理官方实例；不要修改代理本地并发来冒充解决 |

P0 的 `40589` 和 `44741` 只映射到宿主机 loopback，不应改为 LAN 或公网监听。若 P0 闸门失败，继续使用下述宿主 external gateway 方案，或直接改用上面的 cc 路线。

## 备选路线：work.freemodel.dev + 宿主机 WorkBuddy

只有需要 `work.freemodel.dev` 时才用这条路线，它必须在宿主机安装并登录官方 WorkBuddy。使用独立的 compose 文件：`docker compose -f docker-compose.workbuddy.yml up -d`。

`https://work.freemodel.dev` 不是可用普通 Bearer 请求直接调用的公开 OpenAI 端点，它要求官方 WorkBuddy 客户端，通过本机 CodeBuddy ACP gateway 完成请求。

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

### WorkBuddy 路线的启动步骤

```bash
# Git Bash / WSL / Linux / macOS
cp .env.example .env
# 取消 .env 里 work.freemodel.dev 那一组注释，并设置 WORKBUDDY_EXTERNAL_CWD
docker compose -f docker-compose.workbuddy.yml up -d
docker compose -f docker-compose.workbuddy.yml logs -f
```

```powershell
# Windows PowerShell
Copy-Item .env.example .env
# 取消 work.freemodel.dev 那一组注释，WORKBUDDY_EXTERNAL_CWD 例如：
# S:/AIWorker/Freemodel_Proxy/Ciallo_Freemodel_Proxy/workspace
Docker Compose -f docker-compose.workbuddy.yml up -d
Docker Compose -f docker-compose.workbuddy.yml logs -f
```

`docker-compose.workbuddy.yml` 已默认配置：

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

cc 路线（推荐，不需要 host network，也不需要宿主机装东西）：

```bash
docker run -d --name freemodel-proxy \
  -p 127.0.0.1:40589:40589 \
  -e FREEMODEL_BASE_URL=https://cc.freemodel.dev/v1 \
  -e FREEMODEL_TRANSPORT=cc_anthropic \
  -e FREEMODEL_API_KEY="$YOUR_FREEMODEL_KEY" \
  -e PROXY_API_KEY="$YOUR_PROXY_KEY" \
  -v freemodel-proxy-data:/data \
  ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest
```

WorkBuddy 路线：

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

cc 路线只用到前四项，`WORKBUDDY_*` 全部无关：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `FREEMODEL_BASE_URL` | `https://cc.freemodel.dev/v1` | Compose 默认；改成 `work.freemodel.dev` 会强制要求 ACP |
| `FREEMODEL_TRANSPORT` | 按 host 自动推断 | `cc.freemodel.dev` → `cc_anthropic`，`work.freemodel.dev` → `workbuddy_acp`，其它 → `http` |
| `FREEMODEL_API_KEY` | 空 | cc 与 http 路线的上游凭据；cc 路线必填 |
| `PROXY_API_KEY` | 空 | 代理自身 Bearer 鉴权，监听非 loopback 时必填 |
| `PROXY_HOST` | `127.0.0.1` | 代理监听地址 |
| `PROXY_PORT` | `40589` | 代理监听端口 |
| `PROXY_SESSION_STORE` | `/data/sessions.json` | 代理会话元数据 |
| `PROXY_RUNTIME_DIR` | `/data/runtime` | 运行时文件 |
| `WORKBUDDY_SIDECAR_MODE` | `external` | 仅 ACP：Docker 不启动通用 CLI，复用宿主机官方 gateway |
| `WORKBUDDY_ACP_URL` | `http://127.0.0.1:44741` | 仅 ACP：host network 中的宿主机 gateway |
| `WORKBUDDY_EXTERNAL_CWD` | 无 | 仅 ACP external 模式必须设置；宿主机真实工作目录，不是 `/workspace` |
| `WORKBUDDY_ACP_PASSWORD` | 空 | 仅 ACP：gateway 启用 password 模式时设置 |
| `PROXY_DEFAULT_PROJECT` | `/workspace` | 仅 ACP：headerless 客户端的默认项目 |

`FREEMODEL_TRANSPORT` 通常不用手写，按 `FREEMODEL_BASE_URL` 的 host 自动选择即可；显式设置会被校验，`work.freemodel.dev` 只接受 `workbuddy_acp`。

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
# 进程与配置存活检查（不探测上游）
curl -sS http://127.0.0.1:40589/health

# ACP external 模式的 gateway readiness；cc 路线恒为就绪
curl -sS http://127.0.0.1:40589/ready

# 模型列表
curl -sS http://127.0.0.1:40589/v1/models \
  -H "Authorization: Bearer local-proxy"

# cc 路线：真实对话，顺带看 usage
curl -sS http://127.0.0.1:40589/v1/chat/completions \
  -H "Authorization: Bearer local-proxy" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-opus-5","messages":[{"role":"user","content":"只回复 OK"}]}'

# WorkBuddy 路线：真实 ACP 对话
curl -sS http://127.0.0.1:40589/v1/chat/completions \
  -H "Authorization: Bearer local-proxy" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-5.6-sol","messages":[{"role":"user","content":"只回复 OK"}]}'
```

如果设置了 `PROXY_API_KEY`，将上面的 `local-proxy` 换成它的真实值。

cc 路线还有一个不花额度的离线检查，验证协议转换、降级链和 usage 换算：

```bash
bash test/test-cc-smoke.sh
```

## 暴露到局域网

默认配置只监听 `127.0.0.1`。如确需开放给可信局域网：

```dotenv
PROXY_API_KEY=强随机值
```

并把 Compose 中 `PROXY_HOST` 改为 `0.0.0.0`，同时把 `ports:` 的 `127.0.0.1:` 前缀去掉。此时 `PROXY_API_KEY` 不是可选项：代理持有你的上游 key，无鉴权暴露等于把 key 借给同网段任何人。必须同时配置防火墙，不要暴露到公网。

## direct HTTP 回退模式

如果只想使用计费/限额的 `api.freemodel.dev`，可在 `.env` 设置：

```dotenv
FREEMODEL_BASE_URL=https://api.freemodel.dev/v1
FREEMODEL_TRANSPORT=http
FREEMODEL_API_KEY=你的fe_key
```

该模式按 OpenAI 协议原样转发，不做 Anthropic 转换，也不使用 WorkBuddy gateway。

## 故障排查

### cc 路线返回 401 `Invalid token`

`FREEMODEL_API_KEY` 为空、写错或已失效。用 `docker exec -it freemodel-proxy freemodel-workbuddy-proxy key set` 重新写入后 `docker compose restart`。日志和 issue 里不要粘贴 key 本身。

### cc 路线返回 502 `All upstream backends were unavailable`

三个后端都撞上共享容器池占满。这是上游侧的并发限制，与你的账号额度无关，等几十秒重试即可；不要靠调大本地并发来绕。

### 改了 `.env` 里的 key 但没生效

`key set` 会把 key 写进 `/data/config.json`，而 `config.json` 的优先级**高于**环境变量（这是刻意设计，防止外部环境覆盖显式项目配置）。轮换 key 时要么再跑一次 `key set`，要么删掉 `/data/config.json` 里的 `FREEMODEL_API_KEY` 字段。

### 响应里的 `model` 和请求的不一样

上游静默改换了后端，代理如实回显 `message_start.model`。想固定后端就直接请求 `claude-opus-5` / `claude-fable-5` / `claude-haiku-4-5-20251001`。

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

`/proxy/*` 按实际 peer IP 只接受 loopback 请求，不看 `X-Forwarded-For`。cc 路线用的是端口映射（`127.0.0.1:40589:40589`），宿主机的请求经 bridge 网关进入容器，peer IP 是 `172.x.x.1` 而不是 `127.0.0.1`，因此从宿主机直接调 `/proxy/*` 会被拒绝——这是安全设计，不是故障。需要管理路由时进容器调用：

```bash
docker exec freemodel-proxy curl -sS http://127.0.0.1:40589/proxy/diagnostics
```

`/health`、`/ready`、`/v1/*` 不受此限制，从宿主机正常访问。只有 WorkBuddy 路线用 host network，那种情况下 peer IP 才是真 loopback。

## GitHub Actions 自动发布

推送 `beta` 后，workflow 构建 `linux/amd64` 并发布：

- `ghcr.io/murasamecyan/ciallo_freemodel_proxy:beta`
- `ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest`

workflow 使用 `${GITHUB_REPOSITORY,,}` 将仓库名转换为 GHCR 要求的小写。首次发布后需在 GitHub Package settings 将容器包设为 Public。

构建过程不读取 `scr/key.txt`，也没有密钥 build arg。`.dockerignore` 明确排除 `key.txt`、`.env`、`config.json` 和常见私钥格式。
