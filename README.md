# ⚡ Ciallo Freemodel Proxy

来都来了 不点个 ⭐ 再走吗~?

基于 Rust 的 OpenAI 兼容 Freemodel 代理服务。镜像由 GitHub Actions 自动构建发布到 GHCR，**拉取即用，无需安装 Rust 工具链**。

> 📌 代码在 [`beta`](https://github.com/MurasameCyan/Ciallo_Freemodel_Proxy/tree/beta) 分支。本分支只放这份说明。

## 🐳 快速开始

```bash
docker run -d --name freemodel-proxy \
  -p 127.0.0.1:40589:40589 \
  -e FREEMODEL_API_KEY=fe_oa_你的密钥 \
  ghcr.io/murasamecyan/ciallo_freemodel_proxy:latest
```

镜像名全小写是 GHCR 的硬性要求，与仓库名的大小写无关。

> ⚠️ **首次发布后必做**：GHCR 包默认是 **private**，别人拉取会 404。到仓库页面右侧 **Packages** → 点进包 → **Package settings** → 改为 **Public**。

启动后在客户端（Cursor、Continue、Codex CLI、OpenCode 等）填：

- **Base URL**：`http://127.0.0.1:40589/v1`
- **API Key**：任意非空字符串（如 `local-proxy`）
- **模型**：`gpt-5.6-sol`、`gpt-4o`、`opencode-default`

上游凭据始终使用容器内的 `FREEMODEL_API_KEY`，客户端填的值不会被转发。

验证是否正常：

```bash
curl http://127.0.0.1:40589/health
```

## ⚠️ 暴露到局域网前必读

`PROXY_API_KEY` 留空时，代理**接受任何非空客户端密钥**——任何能连到该端口的人都能消耗你的 Freemodel 配额。上面的命令绑定 `127.0.0.1`，仅本机可访问。若要改绑 `0.0.0.0`，先设置密钥：

```bash
-e PROXY_API_KEY="$(openssl rand -hex 32)"
```

## 📦 容器的能力边界

镜像**不含** `codebuddy` CLI，因此只支持 `http` 直连传输：

| | 容器 | 宿主机本地运行 |
| --- | --- | --- |
| 上游端点 | `api.freemodel.dev` | `work.freemodel.dev` |
| 传输方式 | `http` | `workbuddy_acp` |
| `/v1/chat/completions`、`/v1/responses`（含流式） | ✅ | ✅ |
| 交互式 TUI | ❌ | ✅ |
| Codex 函数工具循环 / 视觉输入 | ❌ | ✅ |
| 管理 API `/proxy/*` | 仅容器内（loopback 限制） | ✅ |

需要 TUI、ACP 或 Codex 工具循环时，请 clone `beta` 分支后在宿主机运行 `./start.sh`。

## 📖 更多文档

- [完整部署指南 DOCKER.md](https://github.com/MurasameCyan/Ciallo_Freemodel_Proxy/blob/beta/DOCKER.md) — Compose、持久化、环境变量、故障排查
- [项目完整说明](https://github.com/MurasameCyan/Ciallo_Freemodel_Proxy/blob/beta/README.md) — 架构、传输模式、会话隔离

## 📄 License

MIT
