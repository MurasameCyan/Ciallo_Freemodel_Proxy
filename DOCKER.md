# Docker 部署指南

## 推荐路线：cc.freemodel.dev，只要一个 key

`https://cc.freemodel.dev/v1` 讲 Anthropic Messages 协议，鉴权只看 `Bearer {FREEMODEL_API_KEY}`，**不需要 CodeBuddy 登录，也不需要在宿主机装任何东西**。

代理对客户端同时开放两种入站协议，不用二选一：

- `POST /v1/chat/completions`：OpenAI Chat Completions。代理翻译成 Anthropic Messages 发上游，再翻译回来，现有 OpenAI 客户端不用改。
- `POST /v1/messages`：Anthropic Messages。上游本来就讲这个协议，所以代理几乎原样转发——只改 `model`（外加启用时的 guard 句），`tools`、`tool_result`、图片、`cache_control`、`thinking` 全部原样送达，流式连 `event:` 事件名一起透传。Anthropic SDK 和 Claude Code 走这条路保真度最高。

两个端点共用同一个降级链、同一份 `PROXY_API_KEY` 鉴权。

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

## Web 设定页 `/setup`

不想碰命令行就用浏览器打开 <http://127.0.0.1:40589/setup>。页面能做三件事：

- 填 Freemodel key。**保存立即生效**，不用重启容器；同时写进 `/data/config.json`，容器重建也不丢。上面那条 `key set` 是等价物，区别是它必须重启才生效。
- 显示该抄给客户端的反代地址（OpenAI 与 Anthropic 各一条），带复制按钮。地址取自浏览器地址栏，所以经域名或反向代理访问时给出的也是外部可用的那个，而不是容器内视角的 `127.0.0.1`。
- 显示当前 transport、上游地址和 `/v1/models` 的真实后端列表。

页面本身不含任何秘密，所以 `GET /setup` 不鉴权；读写设定的 `GET`/`POST /setup/key` 与 `/v1/*` 共用同一把 `PROXY_API_KEY`，需要在页面上先填「代理自调用 key」。那把 key 只在页面打开期间留在输入框里，不写 localStorage，刷新后要重填。

设定页刻意不放在 `/proxy/*` 下：那组路由只认真 loopback peer IP，而浏览器经端口映射进来的 peer IP 是 `172.x.x.1`，页面会被自己的安全策略挡在门外（见下面「管理 API 被拒绝」）。

响应只回掩码形式的 key（`fe_o••••page`），不回显全量——它会进浏览器历史、反代日志和用户截图。key 里出现换行、空格或非 ASCII 字符会被直接拒绝：它要拼进 `Authorization` header，放过去只会换来一个和原因无关的报错。

可用模型只有三个真实后端：`claude-opus-5`、`claude-fable-5`、`claude-haiku-4-5-20251001`，`/v1/models` 在这个 transport 下只报这三个。其它 Claude 别名（如 `claude-sonnet-5`）会被上游静默改换成别的后端，代理按 `message_start.model` 把真实后端名回显给客户端，因此响应里的 `model` 可能和你请求的不一样，这是上游行为而不是代理改写。非 `claude-*` 的名字（如客户端默认的 `gpt-4o`）上游不认，代理会直接用 `claude-opus-5` 起步。

上游是共享容器池，占满时返回 500 `Maximum number of running container instances exceeded`，与账号配额无关且随机发生。代理遇到这个错误会沿 `claude-opus-5 → claude-fable-5 → claude-haiku-4-5-20251001` 换后端重试，每次重试前退避 2/4/6 秒，只有全部失败才回 502。4xx（含 401 key 无效）不重试。

退避不能省：这个池是**网关级**上限，换 model 并不会腾出实例，真正让请求成功的是等一会儿。实测零间隔连打必然三连撞满池，退避几秒后同一个请求就能拿到 200。因此单个请求最坏会多花十几秒，这是刻意的。

每次响应都带 `usage`：`prompt_tokens` 含缓存读取，`prompt_tokens_details.cached_tokens` 单列，流式在 finish chunk 上给。用它核对实际消耗，不要依赖官方后台的用量展示。走 `/v1/messages` 时 usage 是上游原样的 Anthropic 形状（`input_tokens` / `output_tokens` / `cache_read_input_tokens`）。

## 上游自带提示词能清到什么程度

cc 的后端是 agent CLI 容器，上游在**容器内部**注入了自己的 harness 提示词。这决定了三件事：

1. 客户端删不掉它。注入不在请求体里，没有字段能关；它也不计入 `usage`。
2. 你的 `system` 对行为有效、对身份无效。指令会被遵守，但问「你是谁」时后端可能自称 Kiro 或 Claude Code，随容器轮换。
3. 实际弄坏客户端的是工具幻觉：harness 让模型以为有文件系统和 shell，于是把 `<function_calls>` / `<tool_call>` 标记和编造的工具结果当正文输出。

`FREEMODEL_PROMPT_GUARD`（默认 `true`）在客户端 `system` **之后**追加一句陈述事实的话来压制第 3 点。追加在后是有意的：越靠后越压得住更靠前的注入，同时客户端指令仍在它之前。按请求里有没有 `tools` 选措辞——带工具的客户端不会被告知「你没有工具」，否则会连合法的 `tool_use` 一起压掉。两种措辞实测 6/6 不再泄漏标记。

```dotenv
# 一个字都不加
FREEMODEL_PROMPT_GUARD=false
```

代理不做响应侧过滤：泄漏形态多变，按关键词删文本会误伤正常内容。想验证当前行为，用 `/v1/messages` 发一句 `列出当前目录` 看它是否老实说自己没有 shell。

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
| `FREEMODEL_PROMPT_GUARD` | `true` | 在客户端 `system` 后追加一句压制上游注入 prompt 的话；`false` 完全不加 |
| `PROXY_API_KEY` | 空 | 代理自身 Bearer 鉴权。绑非 loopback 时**不设就拒绝启动**；镜像固定绑 `0.0.0.0`，所以容器部署一律必填 |
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
# Web 设定页（浏览器打开同一个地址）
curl -sS -o /dev/null -w '%{http_code} %{content_type}\n' http://127.0.0.1:40589/setup

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

# Anthropic 入站协议（Claude Code / Anthropic SDK 走这条）
curl -sS http://127.0.0.1:40589/v1/messages \
  -H "Authorization: Bearer local-proxy" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-opus-5","max_tokens":64,"messages":[{"role":"user","content":"只回复 OK"}]}'

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

并把 `PROXY_BIND` 改为 `0.0.0.0`。此时 `PROXY_API_KEY` 不是可选项，而是启动条件：镜像绑 `0.0.0.0`，没设它代理会直接拒绝启动并说明原因。这一条由代码强制，不靠你记得——空 key 在代码里等于关闭鉴权，而代理持有你的上游 key，无鉴权暴露等于把额度和 `/setup/key` 的写入权限交给所有能连上的人。必须同时配置防火墙，不要暴露到公网。

## direct HTTP 回退模式

如果只想使用计费/限额的 `api.freemodel.dev`，可在 `.env` 设置：

```dotenv
FREEMODEL_BASE_URL=https://api.freemodel.dev/v1
FREEMODEL_TRANSPORT=http
FREEMODEL_API_KEY=你的fe_key
```

该模式按 OpenAI 协议原样转发，不做 Anthropic 转换，也不使用 WorkBuddy gateway。

## 故障排查

### 容器拒绝启动，日志说 `PROXY_HOST=0.0.0.0 不是 loopback，必须同时设置 PROXY_API_KEY`

这不是故障，是安全闸门。空 `PROXY_API_KEY` 在代码里等于**关闭鉴权**，而镜像固定绑 `0.0.0.0`，两者相加就是把你的上游额度和 `/setup/key` 写入权限交给所有能连上这个端口的人。在 `.env` 里设一个强随机值即可：

```bash
openssl rand -hex 24   # 把输出填进 .env 的 PROXY_API_KEY
docker compose up -d
```

客户端此后都要带 `Authorization: Bearer <PROXY_API_KEY>`。**如果你在设置它之前就已经把端口暴露过**，那段时间任何人都能用你的 key，应当去 Freemodel 后台轮换 `FREEMODEL_API_KEY`，再用 `/setup` 页面填入新的。

### `The requested image's platform (linux/amd64) does not match the detected host platform`

早期镜像只发了 `linux/amd64`。现在 `:latest` / `:beta` 是含 `linux/amd64` + `linux/arm64` 的 manifest list，重新拉一次即可：

```bash
docker compose pull
docker compose up -d
```

Docker 会按本机架构自己选层，不需要 `--platform`。如果还是看到这句警告，说明本地缓存的是旧的单架构镜像：

```bash
docker rmi ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest
docker compose pull
```

确认拉到的是多架构镜像：

```bash
docker buildx imagetools inspect ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest
```

应当能看到 `linux/amd64` 与 `linux/arm64` 两条。急用又不想等的话，`docker compose build` 在本机直接编，出来的就是本机架构。

### cc 路线返回 401 `Invalid token`

`FREEMODEL_API_KEY` 为空、写错或已失效。用 `docker exec -it freemodel-proxy freemodel-workbuddy-proxy key set` 重新写入后 `docker compose restart`。日志和 issue 里不要粘贴 key 本身。

### cc 路线返回 502 `All upstream backends were unavailable`

三个后端都撞上共享容器池占满。这是上游侧的并发限制，与你的账号额度无关，等几十秒重试即可；不要靠调大本地并发来绕。

### 改了 `.env` 里的 key 但没生效

`key set` 和 Web 设定页都会把 key 写进 `/data/config.json`，而 `config.json` 的优先级**高于**环境变量（这是刻意设计，防止外部环境覆盖显式项目配置）。轮换 key 时要么在 `/setup` 里重存一次，要么再跑一次 `key set`，要么删掉 `/data/config.json` 里的 `FREEMODEL_API_KEY` 字段。

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

`/health`、`/ready`、`/setup`、`/v1/*` 不受此限制，从宿主机正常访问。只有 WorkBuddy 路线用 host network，那种情况下 peer IP 才是真 loopback。

## GitHub Actions 自动发布

推送 `beta` 后，workflow 发布同时包含 `linux/amd64` 和 `linux/arm64` 的 manifest list：

- `ghcr.io/murasamecyan/ciallo_freemodel_proxy:beta`
- `ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest`

两种架构各在自己的原生 runner 上构建（amd64 用 `ubuntu-latest`，arm64 用 `ubuntu-24.04-arm`），各自跑完冒烟测试后按 digest 推送、不带 tag，最后合成一个 manifest list 才第一次打 tag。因此不存在「latest 只覆盖一半架构」的中间态。CI 最后一步会 inspect 每个 tag 的 manifest，少任何一个架构就直接失败。

arm64 不走 QEMU 模拟：模拟要把整棵 Rust 依赖树重编一遍，耗时以小时计。原生 arm64 runner 对 public 仓库免费。

workflow 使用 `${GITHUB_REPOSITORY,,}` 将仓库名转换为 GHCR 要求的小写。首次发布后需在 GitHub Package settings 将容器包设为 Public。

`:codebuddy-p0` 是例外，只发 `linux/amd64`——它额外打包了官方 CodeBuddy CLI，而那只是一道登录闸门实验。arm64 用户走 `:latest`。

构建过程不读取 `scr/key.txt`，也没有密钥 build arg。`.dockerignore` 明确排除 `key.txt`、`.env`、`config.json` 和常见私钥格式。
