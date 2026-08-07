# ⚡ Freemodel API Proxy

来都来了 不点个⭐再走吗~?

基于 Rust 的 OpenAI 兼容 Freemodel 代理服务。镜像由 GitHub Actions 自动构建并发布到 GHCR。

## 🧪 免宿主安装实验（P0）

`codebuddy-p0` 在 Linux 容器内置官方 CodeBuddy Code 2.133.0，目的是验证官方登录能否在纯 Docker 中完成并持久化。它不会把 Freemodel key 冒充 CodeBuddy 登录凭据，也不会覆盖稳定版标签。

```bash
docker compose -f docker-compose.codebuddy-p0.yml up -d
```

首次尝试打开 `http://127.0.0.1:44741`，或运行：

```bash
docker exec -it ciallo-codebuddy-p0 codebuddy
```

在官方 CLI 输入 `/login`。登录后运行：

```bash
RUN_LIVE_ACP_TEST=1 bash test/test-codebuddy-p0.sh
```

重启验收：

```bash
docker compose -f docker-compose.codebuddy-p0.yml restart
RUN_LIVE_ACP_TEST=1 bash test/test-codebuddy-p0.sh
```

登录、真实 ACP 请求和重启后再次请求必须全部通过，P0 才算可行。不要执行 `docker compose -f docker-compose.codebuddy-p0.yml down -v`，除非你明确要删除 `/data` volume 和登录态。详细验收和故障含义见 [DOCKER.md](DOCKER.md)。

## 🐳 Docker 快速开始（只要一个 key）

默认走 `cc.freemodel.dev`：它讲 Anthropic Messages 协议，鉴权只看 `Bearer {FREEMODEL_API_KEY}`，**不需要登录，也不需要在宿主机装任何东西**。

代理同时提供两种入站协议，客户端用哪种都行：

| 端点 | 协议 | 适用客户端 |
| --- | --- | --- |
| `/v1/chat/completions` | OpenAI Chat Completions | Cursor、Codex、Continue、OpenCode… |
| `/v1/messages` | Anthropic Messages | Claude Code、Anthropic SDK… |

```bash
cp .env.example .env
# 编辑 .env：填 FREEMODEL_API_KEY，并设一个 PROXY_API_KEY
docker compose up -d
```

```powershell
# Windows PowerShell
Copy-Item .env.example .env
# 编辑 .env 后再启动
Docker Compose up -d
```

也可以先不填 key，启动后再写（存进 `/data` 卷，容器重建不丢）：

```bash
docker exec -it freemodel-proxy freemodel-workbuddy-proxy key set
docker compose restart
```

等价的 `docker run`：

```bash
docker run -d --name freemodel-proxy \
  -p 127.0.0.1:40589:40589 \
  -e FREEMODEL_API_KEY="$YOUR_FREEMODEL_KEY" \
  -e PROXY_API_KEY="$YOUR_PROXY_KEY" \
  -v freemodel-proxy-data:/data \
  ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest
```

镜像名全小写是 GHCR 的硬性要求，与仓库原名 `MurasameCyan/Ciallo_Freemodel_Proxy` 的大小写无关。

镜像是多架构的（`linux/amd64` + `linux/arm64`），Apple Silicon、树莓派、Ampere/Graviton 云主机直接 pull 就是原生版本，不需要 `--platform`，也不走 QEMU 模拟。要是看到 `The requested image's platform (linux/amd64) does not match the detected host platform`，那是本地缓存着早期的单架构镜像，`docker compose pull` 重拉一次即可（详见 [DOCKER.md](DOCKER.md#故障排查)）。

客户端 Base URL 填 `http://127.0.0.1:40589/v1`，API key 填你的 `PROXY_API_KEY`（不是 Freemodel key）。Anthropic 客户端同样填这个地址，它会自己拼出 `/v1/messages`；Claude Code 用 `ANTHROPIC_BASE_URL=http://127.0.0.1:40589` 加 `ANTHROPIC_AUTH_TOKEN=$PROXY_API_KEY`。可用模型：`claude-opus-5`、`claude-fable-5`、`claude-haiku-4-5-20251001`。其它 Claude 别名会被上游静默改换，代理如实回显真实后端名，所以响应里的 `model` 可能与请求不同。上游共享容器池占满时返回 500，代理会自动沿这三个后端降级重试，每次重试前退避 2/4/6 秒——这个池是网关级上限，换 model 不腾实例，真正管用的是等一会儿。

每次响应都带 `usage`（含 `cached_tokens`），用它核对真实消耗。

### 🧹 上游自带提示词与 `FREEMODEL_PROMPT_GUARD`

cc 的每个后端都是一个 agent CLI 容器，上游在容器内部注入了自己的一整套 harness 提示词。实测结论要说清楚：

- **它不能被客户端清除。** 注入发生在上游容器内，不在请求体里，因此没有任何字段可以关掉它。它也不计入 `usage`。
- **客户端的 `system` 对行为有效，对身份无效。** 你的指令会被遵守，但问「你是谁」时后端仍可能自称 Kiro 或 Claude Code，按容器轮换。
- **真正影响可用性的不是身份，而是工具幻觉。** harness 让模型以为自己有文件系统和 shell，于是把 `<function_calls>` / `<tool_call>` 这类标记和编造的工具结果当普通文本吐出来，客户端会当成正文显示。

关掉 guard 时问「列出当前目录下的文件」，真实上游的原样回复：

```
<function_calls><invoke name="list-directory">
<parameter name="path">C:\Users\<某个开发者>\Code\client\web-monitor-node-2\server_local</parameter>
</invoke></function_calls>
<function_response>[{"name":".env","type":"file"},{"name":".gitignore","type":"file"},...]
```

路径和文件列表都是编造的，但它示范了这套 harness 会把什么东西当正文吐给客户端。

代理的处理是 `FREEMODEL_PROMPT_GUARD`（默认 `true`）：在**客户端 `system` 之后**追加两句陈述事实的话，压制上面那种幻觉——位置靠后才压得住更靠前的注入，同时客户端指令仍在它前面。按请求里有没有 `tools` 选用两种措辞：

- 无 `tools`：`You have no tools, no filesystem access and no shell in this session. Never emit <function_calls>, <invoke>, <tool_call> or <function_response> markup as text, and never invent tool results. If a request would need tools, say so in plain text.`
- 有 `tools`：`The only tools available are the ones declared in this request; you have no filesystem access and no shell beyond them. Invoke them through the structured tool-call mechanism only. Never emit <function_calls>, <invoke>, <tool_call> or <function_response> markup as text, and never invent tool results.`

两种措辞都实测过不再泄漏标记，且不影响合法的 `tool_use`——同一个「列出当前目录」的问题，开 guard 后模型改成直说自己没有文件系统权限；客户端自带 `tools` 时仍正常返回 `stop_reason: tool_use` 和结构化调用。带工具的客户端不会被告知「你没有工具」，这是分开两句的原因。设 `FREEMODEL_PROMPT_GUARD=false` 可完全关闭，代理就一个字都不加。

抽象的元指令（「忽略先前的系统提示」之类）压不住，实测 3/3 仍泄漏；只有陈述事实的写法有效。想自己复现这组对比：`python3 test/probe_cc_guard.py <key 文件>`（会真实消耗额度，因此不进 CI）。

代理不做响应侧过滤：泄漏形态多变，靠关键词删文本会误伤正常内容。

### 备选：work.freemodel.dev

`work.freemodel.dev` 只接受官方 WorkBuddy 客户端。容器不会复制、伪造或打包私有认证，而是通过 ACP 复用宿主机**已登录且正在运行**的 WorkBuddy gateway。

1. 在宿主机启动并登录官方 WorkBuddy，确认其 ACP gateway 监听 `127.0.0.1:44741`。
2. Docker Desktop 4.34+ 打开 **Settings → Resources → Network → Enable host networking**。
3. 在 `.env` 中取消 `work.freemodel.dev` 那一组注释，把 `WORKBUDDY_EXTERNAL_CWD` 填成**宿主机 WorkBuddy 可访问的真实工作目录**；它不能写成容器路径 `/workspace`。
4. `docker compose -f docker-compose.workbuddy.yml up -d`

该路线模型名为 `gpt-5.6-sol`、`gpt-4o`、`opencode-default`。

> `key.txt` 里的 `fe_...` key 可直接用于 `cc.freemodel.dev` 和 `api.freemodel.dev`，但实测不能代替 WorkBuddy/CodeBuddy 登录；把它作为 `CODEBUDDY_AUTH_TOKEN` 或 `CODEBUDDY_API_KEY` 都不能获得 `work.freemodel.dev` 权限。

详细部署、gateway 端口确认和故障排查见 [DOCKER.md](DOCKER.md)。

### 容器与本地运行的差异

| | 容器默认 | 本地默认 |
| --- | --- | --- |
| `FREEMODEL_BASE_URL` | `https://cc.freemodel.dev/v1` | `https://work.freemodel.dev/v1` |
| `FREEMODEL_TRANSPORT` | `cc_anthropic` | `workbuddy_acp` |
| `WORKBUDDY_SIDECAR_MODE` | `external`（仅 ACP 路线用到） | `managed`，按会话启动官方 CLI sidecar |

管理类路由（`/proxy/sessions`、`/proxy/diagnostics`）按实际 peer IP 限制为 loopback。容器用端口映射时，宿主机的请求经 bridge 网关进来，peer IP 不是 `127.0.0.1`，这些路由会被拒绝；需要时用 `docker exec freemodel-proxy curl http://127.0.0.1:40589/proxy/diagnostics` 在容器内调用。`/health`、`/ready`、`/v1/*` 不受影响。Compose 默认只把端口绑到宿主机 `127.0.0.1`；如果改成 `0.0.0.0`，必须同时设置强随机 `PROXY_API_KEY`——代理持有你的上游 key，无鉴权暴露等于把 key 借给同网段任何人。

---

## 🌟 Features

- ⚡ **OpenAI Compatibility**: Emulates `/v1/chat/completions`, `/v1/models`, and `/v1/responses`.
- 🅰️ **Anthropic Messages Inbound**: Serves `/v1/messages` for Anthropic-native clients, streaming included. On `cc_anthropic` it is a near pass-through, so `tools`, `tool_result`, images, `cache_control`, and `thinking` reach the upstream unchanged; other transports are bridged onto the OpenAI dispatch path.
- 🧹 **Injected-Prompt Guard**: Appends two factual sentences after the client `system` to stop the upstream harness from emitting fake tool-call markup as text. Toggle with `FREEMODEL_PROMPT_GUARD`.
- 💬 **Interactive Rust TUI**: Full-screen Ratatui workspace with guided setup, validated live streaming, session/model/project pickers, retry/edit/cancel controls, search, diagnostics, logs, preferences, and masked key setup.
- 🔑 **Automatic Key Resolution**: Auto-detects and persists API keys locally in `config.json` or reads existing keys from `~/.codex/auth.json`.
- 🔄 **Native Incremental Streaming**: Forwards direct HTTP and WorkBuddy ACP deltas immediately for both Chat Completions and Responses API clients.
- 🧰 **Function Tool Translation**: Converts Responses function definitions, calls, and outputs to and from Chat Completions tool-call format.
- 🛡️ **Explicit Stream Failures**: Reports truncated upstream streams as `response.failed` instead of silently closing before completion.
- 🔐 **Official WorkBuddy ACP Transport**: Routes the protected `work.freemodel.dev` endpoint through an active official WorkBuddy gateway instead of imitating private client authentication.
- 🧭 **Dynamic Gateway Discovery**: Finds live gateways from `~/.workbuddy-ai/sessions`, skips stale registrations, and rotates retryable failures across candidates.
- 🔒 **Concurrent Request Isolation**: Serializes ACP sessions per gateway to prevent prompt/response crossover while allowing separate gateways to operate concurrently.
- 🧩 **Proxy-Owned Sessions**: Keeps terminal and Codex conversations in a separate proxy store instead of reusing the active WorkBuddy GUI conversation.
- 📁 **Project-Aware TUI**: Select a project, create a fresh proxy session, or reopen an older proxy-only session with saved history.
- 🛰️ **Dedicated Official Sidecars**: Starts one loopback-only official CodeBuddy CLI gateway per active proxy session, stops it after an idle timeout, and relaunches it on demand.

---

## 🔀 Transport Configuration

The proxy supports three transports, and it picks one automatically from the `FREEMODEL_BASE_URL` host, so you normally do not set `FREEMODEL_TRANSPORT` at all:

- `cc_anthropic` (auto for `cc.freemodel.dev`): Translates OpenAI Chat Completions to Anthropic Messages against `https://cc.freemodel.dev/v1/messages`, and forwards inbound `/v1/messages` almost verbatim — only `model` is rewritten (plus the guard sentence when enabled), because the upstream already speaks Anthropic and any translation would drop `tools`, `tool_result`, images, `cache_control`, or `thinking`. Auth is only `Bearer {FREEMODEL_API_KEY}` — no login, no local gateway, nothing installed on the host. On upstream HTTP 500 `Maximum number of running container instances exceeded` (shared container pool, unrelated to your quota) the proxy retries down `claude-opus-5 → claude-fable-5 → claude-haiku-4-5-20251001`; 4xx never retries. The real backend name is read from `message_start.model`, because upstream silently swaps models.
- `workbuddy_acp` (auto for `work.freemodel.dev`, and required there): Official WorkBuddy ACP for the logical WorkBuddy service at `https://work.freemodel.dev/v1`. The OpenAI-style service route is `https://work.freemodel.dev/v1/chat/completions`, but the proxy deliberately does **not** POST to that protected URL. It launches or discovers an authenticated local CodeBuddy ACP gateway and exchanges ACP messages through loopback `/api/v1/acp`.
- `http` (auto for anything else): Direct OpenAI-compatible passthrough. Do not use this transport with `work.freemodel.dev`.

The Docker default is equivalent to:

```json
{
  "FREEMODEL_BASE_URL": "https://cc.freemodel.dev/v1",
  "FREEMODEL_TRANSPORT": "cc_anthropic"
}
```

You may place those values in the ignored local `config.json`, but they do not need to be repeated. Note that `config.json` takes precedence over environment variables by design, so a key written by `key set` wins over a later `FREEMODEL_API_KEY` in the environment. For a deliberate generic HTTP upstream, configure its base URL (normally ending in `/v1`, without `/chat/completions`) and set `FREEMODEL_TRANSPORT` to `http`; the proxy appends `/chat/completions` only on that direct-HTTP path.

For the default protected service, local execution uses `WORKBUDDY_SIDECAR_MODE=managed` and launches the official CodeBuddy CLI as a dedicated gateway for each proxy session. Docker uses `WORKBUDDY_SIDECAR_MODE=external` and connects to the already authenticated official WorkBuddy gateway at `WORKBUDDY_ACP_URL`; it never copies or imitates private HTTP authentication.

Optional ACP settings are `WORKBUDDY_ACP_TIMEOUT` and `WORKBUDDY_ACP_MAX_ATTEMPTS`. Session-isolation settings are:

- `WORKBUDDY_SIDECAR_MODE`: `managed` starts a local official CLI; `external` uses `WORKBUDDY_ACP_URL` without launching a sidecar.
- `WORKBUDDY_EXTERNAL_CWD`: external 模式必填；填写 gateway 所在宿主机可见的真实工作目录。host networking 只提供网络可达性，不映射容器 `/workspace` 路径。
- `WORKBUDDY_CLI_PATH`: optional path to the official `codebuddy` executable in managed mode. If omitted, the proxy resolves `codebuddy` from `PATH`; no machine-specific path is compiled in.
- `PROXY_DEFAULT_PROJECT`: fallback project for clients that cannot send a project header.
- `PROXY_SESSION_STORE`: proxy-owned JSON metadata and TUI history store.
- `PROXY_RUNTIME_DIR`: sidecar logs and runtime files.
- `PROXY_SIDECAR_STARTUP_TIMEOUT`: maximum sidecar startup wait.
- `PROXY_SIDECAR_IDLE_TIMEOUT`: seconds before an inactive sidecar is stopped; its session metadata remains reusable.
- `PROXY_MAX_HISTORY_TURNS`: number of user/assistant turn pairs retained for the TUI.
- `FREEMODEL_PROMPT_GUARD`: `true` by default. Appends the guard sentence described above after the client `system`. Accepts `1/0`, `true/false`, `yes/no`, `on/off`, or a real JSON boolean in `config.json`. Set it to `false` to send the client prompt untouched.
- `PROXY_API_KEY`: optional Bearer key required by `/v1/models`, `/v1/chat/completions`, `/v1/messages`, and `/v1/responses`. Loopback-only management and health routes remain available locally. When enabled, the proxy uses `FREEMODEL_API_KEY` for the direct upstream instead of forwarding the proxy credential.

`cc_anthropic` ignores every `WORKBUDDY_*` and sidecar setting; it needs only `FREEMODEL_BASE_URL` and `FREEMODEL_API_KEY`. Its responses always carry `usage`, with cache reads folded into `prompt_tokens` and broken out under `prompt_tokens_details.cached_tokens`; streaming attaches them to the finish chunk. `/v1/models` reports only the three real cc backends on this transport.

`WORKBUDDY_ACP_URL` and `WORKBUDDY_EXTERNAL_CWD` are required when `workbuddy_acp` uses external mode. `WORKBUDDY_ACP_CWD` and `WORKBUDDY_ACP_PASSWORD` remain available for manual ACP configuration. `/health` is a process/configuration liveness check; use `/ready` or a real ACP request to verify that the external gateway is reachable. Never commit gateway passwords or API keys.

### Reliability and error semantics

- Authentication and protocol/configuration errors stop immediately; transient network, timeout, capacity, and explicit refusal failures can be retried within `WORKBUDDY_ACP_MAX_ATTEMPTS`, but only before the first response delta has been sent.
- User cancellation and `max_tokens` terminal results are not automatically retried.
- Cancelling a downstream request sends `session/cancel` when an ACP session exists, then closes the ACP connection.
- Errors detected before streaming preserve their HTTP status where available instead of being presented as successful assistant text.
- Mid-stream Chat failures are emitted as an OpenAI-style SSE `error` object and do not emit `[DONE]` as success.
- Mid-stream Responses failures emit exactly one `response.failed`; successful Responses streams emit exactly one `response.completed`.
- Malformed JSON, invalid SSE payloads, premature EOF, or `[DONE]` without a finish reason are treated as explicit failures.

---

## 🧩 Proxy Session Isolation

The proxy session ID is independent from WorkBuddy GUI conversation IDs. Session records are stored only in `PROXY_SESSION_STORE`; the proxy does not edit `~/.workbuddy-ai/app/sessions.json` and never terminates GUI-managed processes.

### TUI workflow

Running `./start.sh` opens the Rust hybrid TUI:

1. securely prompts without echo for a Freemodel API key only when no usable key can be resolved from project configuration, inherited environment fallback, or Codex auth; the key is saved in the ignored project `config.json` with owner-only (`0600`) permissions and used immediately;
2. verifies or starts the compatible local Rust proxy and reports actionable startup failures;
3. asks for a project directory with recent-project choices;
4. lists only proxy-owned sessions for that project and rejects invalid selections;
5. lets you create a new session or reopen an old one;
6. restores saved history and routes through the selected session's dedicated sidecar;
7. opens a full-screen, resize-safe chat workspace with wrapped multiline transcripts, multiline input, validated streaming, cancellation, retry/edit-resend, transcript search, model and project selection, diagnostics, a bounded sanitized proxy-log view, and persisted non-secret preferences;
8. uses full-transcript replacement after retry/edit so corrected turns replace saved history rather than duplicating old turns.

Press `F1`, `Ctrl+K`, or type `/help` for all shortcuts and commands. The composer supports normal text editing with Left/Right, selection with Shift+Arrow or `Ctrl+A`, and `Ctrl+C`/`Ctrl+X`/`Ctrl+V` for selected input. The `?` character is regular input. Up/Down moves between multiline or wrapped rows first, then browses sent-prompt history while preserving the current draft. Common actions include `Ctrl+O` session picker, `Ctrl+P` project switch, `Ctrl+M` model picker, `Ctrl+R` retry, `Ctrl+E` edit/resend, `Esc` cancel/close, and `Ctrl+Q` safe exit. Session commands include `/new`, `/sessions`, `/switch`, `/rename`, `/clear`, and `/delete`; destructive commands require confirmation.

Sidecar processes are temporary. The first request for a session can take up to the configured `PROXY_SIDECAR_STARTUP_TIMEOUT` (90 seconds by default) while the official CLI initializes; later requests reuse the healthy sidecar. An idle sidecar is stopped after `PROXY_SIDECAR_IDLE_TIMEOUT`, while the session title, project, and history stay available. Selecting or addressing that session again starts a new sidecar automatically. The proxy writes session metadata and sidecar log files with owner-only permissions (`0600`), keeps runtime directories private (`0700`), and launches each sidecar with a minimized environment that excludes proxy credentials, provider API keys, gateway passwords, and dynamic-loader injection variables.

### Codex and OpenAI-compatible clients

Use the normal base URL:

```text
http://127.0.0.1:40589/v1
```

For deterministic routing, send both headers:

```http
X-WorkBuddy-Session: proxy-<session-id>
X-WorkBuddy-Project: /absolute/path/to/project
```

Create and inspect proxy-only sessions through the loopback management API:

```bash
curl -sS -X POST http://127.0.0.1:40589/proxy/sessions \
  -H "Content-Type: application/json" \
  -d '{"project":"/absolute/path/to/project","title":"Codex work"}'

curl -sS "http://127.0.0.1:40589/proxy/sessions?project=/absolute/path/to/project"
```

If a client cannot set custom headers, omit them. The proxy derives a stable automatic session from the canonical project and the earliest system/developer/user context, and returns the resolved ID in `X-WorkBuddy-Session`. Set `PROXY_DEFAULT_PROJECT` correctly for headerless clients, or start server-only mode with an explicit workspace:

```bash
./start.sh --server-only --project /absolute/path/to/project
```

The selected path is canonicalized and must already be a directory. Successful Chat Completions and Responses requests return both `X-WorkBuddy-Session` and `X-WorkBuddy-Project`; `GET /proxy/diagnostics` also reports `default_project`, so you can verify where a headerless client is routed. For reliable resume behavior across client restarts, explicit session headers are preferred.

Management routes are loopback-only:

- `GET /proxy/sessions`
- `POST /proxy/sessions`
- `GET /proxy/sessions/{session_id}`
- `PATCH /proxy/sessions/{session_id}`
- `POST /proxy/sessions/{session_id}/history`
- `PUT /proxy/sessions/{session_id}/history`
- `DELETE /proxy/sessions/{session_id}/history`
- `DELETE /proxy/sessions/{session_id}`
- `GET /proxy/diagnostics`

Deleting a session stops only a process whose PID and command line match the proxy-owned sidecar marker.

---

## 🚀 Quick Start

### 1. Launch the hybrid TUI and proxy

```bash
./start.sh
```

On the first interactive launch, if no usable API key is already available, the TUI asks for it securely without displaying the entered characters and stores it only in the ignored local `config.json` with `0600` permissions. Later launches reuse the saved key and do not prompt again.

Or build and run it directly:

```bash
cargo run --release -- tui
```

### 2. Run the server only

```bash
./start.sh --server-only
# or
cargo run --release -- server
```

Configure the API key without exposing it as a process argument:

```bash
cargo run --release -- key set
```

---

## 🔌 Connecting Your Apps (Cursor, Codex, Continue, OpenCode)

- **Base URL on this machine**: `http://127.0.0.1:40589/v1` (use `/v1`, not `/v1/chat/completions`). The launcher prints this URL, and the TUI sidebar and `/diagnostics` command show it while running.
- **API Key**: use the exact configured `PROXY_API_KEY` when proxy authentication is enabled. When `PROXY_API_KEY` is blank, the local proxy accepts any non-empty client placeholder required by the app and always authenticates upstream with its private configured `FREEMODEL_API_KEY`; the client value is never forwarded to Freemodel. Do not expose the upstream key in client settings.
- **Supported Models**: `gpt-5.6-sol`, `gpt-4o`, `opencode-default`

Example Codex CLI configuration in `~/.codex/config.toml`:

```toml
model = "gpt-5.6-sol"
model_provider = "freemodel_local"

[model_providers.freemodel_local]
name = "Freemodel local proxy"
base_url = "http://127.0.0.1:40589/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
```

Set `OPENAI_API_KEY` to the configured `PROXY_API_KEY`. When proxy authentication is disabled, use a non-empty local placeholder such as `local-proxy` if the client requires a credential; the proxy ignores that value for upstream authentication and uses its private `FREEMODEL_API_KEY`. This provider configuration was smoke-tested with Codex CLI `0.146.0`. A `429 Credits exhausted` response comes from the logged-in WorkBuddy account, not from local proxy connectivity.

### Codex project files and images

Open Codex App on the intended project, or launch Codex CLI from that directory. Codex's own filesystem and image tools—not the proxy—read files under the workspace and return only the requested results through the Responses function-call loop. For a client that cannot send `X-WorkBuddy-Project`, start the proxy with the same path using `./start.sh --server-only --project /absolute/path/to/project` (or set `PROXY_DEFAULT_PROJECT`). Verify it through `X-WorkBuddy-Project` or `/proxy/diagnostics`.

The proxy deliberately does **not** scan, enumerate, or upload the project directory. It does not add a proxy-owned `read_file` endpoint and does not grant access outside the selected workspace. Direct HTTP Responses requests preserve Codex function definitions, calls, and outputs so Codex can perform file operations under its own sandbox and permission policy.

For vision input, direct HTTP accepts OpenAI Responses `input_image` blocks whose `image_url` is HTTP(S) or a `data:image/...` URL and converts them without dereferencing the URL. Existing Chat Completions `image_url` blocks are preserved. Bare local paths, `file://` URLs, and `file_id` references return an explicit `400` instead of being silently discarded: let Codex use its normal image/file tool to read the workspace image, or submit encoded image content. The proxy never opens or base64-encodes local images automatically.

The protected WorkBuddy ACP transport remains project-scoped—its sidecar and ACP session use the canonical project as their working directory—but client-supplied function tools are not supported by that transport. Use the direct HTTP transport for the complete Codex Responses tool loop and encoded vision inputs.

### Skills, tools, and images compatibility

| Capability | Direct HTTP (`http`) | WorkBuddy ACP (`workbuddy_acp`) | cc Anthropic (`cc_anthropic`) |
| --- | --- | --- | --- |
| `/v1/chat/completions` text, streaming and not | Yes | Yes | Yes |
| `/v1/messages` text, streaming and not | Yes; bridged onto the OpenAI upstream | Yes; bridged onto ACP | Yes; near pass-through, highest fidelity |
| `/v1/responses` text | Yes | Yes | No; returns an explicit `400` — use `/v1/chat/completions` |
| Client function-tool loop | Yes | No; rejected explicitly | `/v1/messages` yes, unmodified; `/v1/chat/completions` text only |
| `/v1/messages` `tool_result` history | Flattened to text; the tool output survives, its structure does not | Flattened to text | Preserved unchanged |
| Codex/WorkBuddy skills | Executed by the client through its function-tool loop; the proxy transports calls and results | Sidecar-internal skills may be available, but they are not exposed as a transparent client tool loop | Not applicable |
| Vision/image input | HTTP(S) and `data:image/...` URLs | Not supported through the ACP text transport | No; multipart content is flattened to text |
| Bare local image paths | No; the client must read/encode them | No | No |
| Image generation | No image-generation endpoint or output-event translation | No | No |

A skill is not installed or executed by the proxy itself. Codex or WorkBuddy owns skill discovery, permissions, and execution; the direct transport preserves the Responses function calls needed for that workflow. Image understanding (vision input) must not be confused with image generation, which this proxy does not implement.

`PROXY_MAX_SIDECARS` controls only proxy-owned local CodeBuddy gateway processes and defaults to `16`. Increase it only if the machine has sufficient memory and file descriptors. It does **not** control the WorkBuddy service's agent/container `max_instances` quota. An upstream "Maximum number of running container instances exceeded" response must be resolved by freeing/waiting for upstream instances or changing the official service/account configuration; changing `PROXY_MAX_SIDECARS` cannot raise that quota.

The default `PROXY_HOST=127.0.0.1` is intentionally available only on the same computer. For another trusted device on your LAN, set both `PROXY_HOST=0.0.0.0` and a strong non-empty `PROXY_API_KEY`, allow TCP port `40589` only on the private firewall zone, and use `http://<this-computer-LAN-IP>:40589/v1`. Do not expose an unauthenticated wildcard bind to a LAN or the public internet.

---

## 🧪 Testing

Run the Rust unit, protocol, API, and TUI suite:

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --release --all-targets
```

The Python implementation remains temporarily available only as a differential compatibility oracle until the wider Rust migration reaches final cutover.
