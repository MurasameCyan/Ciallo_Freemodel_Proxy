# Docker 内置 CodeBuddy 与 Web Setup 设计

**日期：** 2026-08-07  
**状态：** 方案 A 已获用户选择；按 P0 可行性闸门分阶段实施

## 1. 目标与边界

最终体验是：用户只安装 Docker，完成官方 CodeBuddy 登录后，在代理的 Web Setup 中生成本地调用 key，并复制 OpenAI-compatible Base URL 到其他客户端。

必须区分两个产品：

- WorkBuddy Desktop 只有 Windows/macOS GUI 包，不适合 Linux 容器。
- Linux 容器内置的是官方 `@tencent-ai/codebuddy-code@2.133.0` headless CLI。它以 `--serve` 启动 HTTP/SSE ACP gateway；不是第三方伪造的 WorkBuddy 客户端。

`FREEMODEL_API_KEY` 不能被当作 CodeBuddy 登录凭据。此前实测把 `fe_...` key 写入 `CODEBUDDY_AUTH_TOKEN`、`CODEBUDDY_API_KEY` 或配合 `CODEBUDDY_BASE_URL=https://work.freemodel.dev/v1` 均未获得权限。因此方案 A 的上游登录必须走 CodeBuddy 官方登录流程；Web 中的 Freemodel key 仅可用于明确选择的 direct HTTP 模式，不能解锁 `work.freemodel.dev`。

代理不得逆向、复制或伪造官方私有认证，不承诺免费、无限量或绕过 quota/max_instances。

## 2. 分阶段交付

### P0：登录与持久化可行性镜像

先发布隔离的实验标签 `codebuddy-p0`，不覆盖 `latest`/`beta`。镜像包含 Rust 代理与固定版本 CodeBuddy Code 2.133.0，并将 CodeBuddy 的 `HOME` 持久化到 `/data/codebuddy-home`。

P0 需要用户验证：

1. 官方 CodeBuddy Web UI 或容器内交互 CLI 能完成 `/login`；
2. 登录回调在无宿主安装的 Docker 场景可完成；
3. 重启容器后登录态仍可使用；
4. Rust 代理通过容器内 `/api/v1/acp` 能取得一条真实模型响应；
5. 日志、镜像层、Git 历史和配置输出均不包含凭据。

P0 失败时停止方案 A，不进入 Web Setup 实现；现有 external WorkBuddy 模式继续作为已验证回退。不得用 Freemodel key 冒充官方登录来绕过闸门。

### P1：安全 Web Setup

只有 P0 五项全部通过后才实现 `/setup`。首次访问页面完成以下操作：

- 显示 CodeBuddy gateway 可达和官方登录状态，但不回显 token；
- 提供官方登录入口/指令；
- 使用 OS CSPRNG 生成 256-bit 本地 `PROXY_API_KEY`，只在生成响应中显示一次；
- 显示并复制 `http://127.0.0.1:40589/v1`；
- 保存 allowlist 配置到 `/data/config.json`，响应明确 `restart_required`；
- direct HTTP 模式才允许录入 Freemodel key，并明确它不适用于受保护的 WorkBuddy ACP 路径。

## 3. P0 架构

实验镜像使用一个容器内的两个进程：

1. CodeBuddy gateway：
   `codebuddy --serve --host 0.0.0.0 --port 44741`
2. Rust proxy：
   `freemodel-workbuddy-proxy server`

Rust proxy 使用现有 external transport：

- `WORKBUDDY_SIDECAR_MODE=external`
- `WORKBUDDY_ACP_URL=http://127.0.0.1:44741`
- `WORKBUDDY_EXTERNAL_CWD=/workspace`

这里的 external 是“相对 Rust 进程外部”，gateway 仍在同一容器。这样复用现有、已有集成测试的 ACP 客户端和 readiness，而不是同时改写 sidecar 生命周期。

入口脚本负责：

- 创建 `/data/codebuddy-home`、`/data/runtime`、`/workspace`；
- 以非 root 用户启动两个进程；
- 捕获 `TERM`/`INT` 并同时停止两个子进程；
- 任一关键子进程退出时使容器退出；
- 不打印环境变量、token 或配置文件内容。

## 4. 网络与登录

实验 Compose 仅将端口绑定到宿主 loopback：

- `127.0.0.1:40589:40589`：代理 API；
- `127.0.0.1:44741:44741`：官方 CodeBuddy Web UI/gateway。

`44741` 在 P0 中是验证入口，不应映射到 LAN 或公网。CodeBuddy 2.133.0 没有 `--auth` CLI 参数；配置使用真实存在的 `CODEBUDDY_GATEWAY_AUTH`。P0 在宿主 loopback 映射下使用 `none`，避免捏造未验证的 password 参数链路；P1 前必须根据真实登录试验决定是否启用官方 gateway password。

首选登录路径是在官方 Web UI 中执行 `/login`。若 2.133.0 Web UI 不提供该命令，P0 允许执行：

```bash
docker exec -it ciallo-codebuddy-p0 codebuddy
```

然后在容器中的官方交互 CLI 输入 `/login`，由宿主浏览器完成官方回调。这不要求用户在宿主机安装 WorkBuddy/CodeBuddy，但属于 P0 需要实测的交互路径。

所有 CodeBuddy 状态存入命名卷下的 `/data/codebuddy-home`。删除 volume 等同清除登录态。

## 5. P1 组件边界

P1 预计新增：

- `src/web_setup.rs`：窄 DTO、页面/状态/保存/生成 key handlers；不承载 ACP 逻辑。
- `web/setup.html`：`include_str!` 编译嵌入的单文件页面，不引入前端构建链。
- `src/config.rs`：将现有 `save_api_key` 抽为带文件锁的 allowlist 原子更新器；不接受任意 JSON key。
- `src/server.rs`：只挂载 setup routes，并复用实际 peer loopback guard。

`Config` 不直接序列化到响应。`SetupStatus` 只返回 configured 布尔值、非秘密生效字段、gateway/login 状态和 `restart_required`。

## 6. Web Setup 安全模型

即使仅 loopback，恶意网页仍可能向本机服务发起请求。因此 P1 必须同时满足：

- 使用 `ConnectInfo<SocketAddr>` 验证实际 peer IP 为 loopback，不信任 `X-Forwarded-For`；
- 校验 `Host` 为允许的 loopback host/port；
- 拒绝非同源 `Origin`，对缺失 Origin 的写请求要求启动期 CSRF token；
- 写接口只接受 `application/json`，使用小于全局 API 限制的独立 body limit；
- 设置 `Cache-Control: no-store`、CSP、`frame-ancestors 'none'`、`X-Content-Type-Options: nosniff`；
- 已存在 `PROXY_API_KEY` 时，所有管理写操作还要求该 Bearer key；
- 上游 key、官方 token、gateway password 和代理 key不进入 URL、localStorage、日志或状态响应；
- 代理 key仅在生成响应中显示一次，配置页刷新后只显示“已配置”。

配置保存使用文件锁、同目录临时文件、`sync_all`、原子替换和 Unix `0600`。容器内运行于 Linux；不把该权限断言宣传为 Windows 原生 DACL 保证。

## 7. 数据流

### P0 登录

浏览器/容器 CLI → 官方 `/login` → 官方认证服务 → `/data/codebuddy-home` 登录态 → CodeBuddy gateway → loopback ACP → Rust proxy → OpenAI-compatible client。

### P1 Setup

浏览器 → `/setup/api/status`（脱敏）→ `/setup/api/generate-key`（一次显示）→ allowlist 配置原子写入 `/data/config.json` → 重启容器 → 客户端使用 Base URL + 代理 key。

`PROXY_API_KEY` 与上游身份严格分离：前者只鉴权代理公开 API，后者由官方 CodeBuddy 自行管理。

## 8. 错误处理

- gateway 未启动：`/ready` 返回 502，并提示检查 CodeBuddy 日志。
- gateway 可达但未登录：真实模型请求保留官方 401/403 语义；文档引导 `/login`，不建议填写 Freemodel key。
- 登录回调在 Docker 失败：P0 判定不通过，停止 P1。
- 登录态重启后丢失：P0 判定不通过，检查实际配置目录后再决定是否能修复。
- 上游 quota/capacity/max_instances：明确为官方账号限制，不自动重试成“成功”。
- 配置并发写或 fsync/rename 失败：保留旧配置并返回错误，不部分写入。
- 子进程退出：入口脚本关闭另一子进程并使容器以非零状态退出。

## 9. 测试与发布闸门

P0 自动测试：

- shell 测试用 fake gateway/fake proxy 验证启动、参数、环境、信号传播和退出语义；
- Docker build 验证固定包版本、`codebuddy --version`、非 root 用户和健康检查；
- Compose config 验证仅 loopback 端口映射与 `/data` volume；
- 工作树和全部可达 Git commit 做 key pattern/exact-key scan；
- P0 workflow 只推 `ghcr.io/murasamecyan/ciallo_freemodel_proxy:codebuddy-p0`。

P0 手工验收必须在真实 Docker 环境完成登录、重启和一条真实 ACP 请求。CI 不保存用户凭据，也不执行带真实 key 的 Actions。

P1 自动测试：

- 非 loopback、伪造 XFF、foreign Origin、非法 Host、非 JSON、缺 CSRF 全部拒绝；
- status/日志/响应不泄露任何秘密；
- CSPRNG key 长度与格式；
- 一次显示语义；
- allowlist 配置、并发保存、原子性与 `0600`；
- 已配置后要求管理认证；
- 重启后代理 Bearer 鉴权和 ACP 路由集成测试。

## 10. 许可证与再分发

官方 CNB 仓库 `LICENSE.txt` 是 MIT License，明确授予 use/copy/modify/publish/distribute/sublicense/sell 权利，条件是副本或实质部分包含版权与许可声明。镜像和仓库中的 third-party notices 必须保留：

```text
Copyright Copyright 2023 Tencent Cloud
```

并附完整 MIT 文本。镜像固定 2.133.0 和 npm integrity，避免不可复现的 `latest` 安装。第三方依赖仍按其各自许可证处理。
