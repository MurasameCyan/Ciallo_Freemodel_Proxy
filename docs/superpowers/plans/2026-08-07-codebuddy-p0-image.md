# CodeBuddy P0 实验镜像实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 发布隔离的 `:codebuddy-p0` 实验镜像，让用户在不安装宿主 WorkBuddy/CodeBuddy 的情况下验证官方 CodeBuddy 2.133.0 在纯 Docker 中的登录、重启持久化和真实 WorkBuddy ACP 请求。

**架构：** 新镜像在同一容器内运行官方 CodeBuddy `--serve` gateway 和现有 Rust proxy。Rust proxy 保持已验证的 `external` ACP 路径，但 URL 改为容器 loopback；CodeBuddy 的 HOME 存在 `/data/codebuddy-home`。P0 不实现 Web Setup，也不改写 `latest`/`beta` 镜像；真实登录成功是 P1 的硬闸门。

**技术栈：** Rust/Axum 现有二进制、Bash 进程监督、Node.js 20、`@tencent-ai/codebuddy-code@2.133.0`、Docker Compose、GitHub Actions、GHCR。

---

## 文件结构

- 创建 `docker/codebuddy-p0-entrypoint.sh`：启动并监督 CodeBuddy gateway 与 Rust proxy，不打印秘密。
- 创建 `Dockerfile.codebuddy-p0`：构建 Rust，安装固定版本官方 CodeBuddy，并形成 P0 镜像。
- 创建 `docker-compose.codebuddy-p0.yml`：仅向宿主 loopback 暴露代理和官方 gateway，持久化 `/data`。
- 创建 `THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt`：满足官方 MIT 再分发条件。
- 创建 `test/test-codebuddy-p0-entrypoint.sh`：不用 Docker 即可测试入口脚本参数、环境、退出和信号行为。
- 创建 `test/test-codebuddy-p0-files.sh`：静态验证镜像/Compose 的固定版本、loopback、volume 和无 key 构建约束。
- 创建 `test/test-codebuddy-p0.sh`：面向用户的构建、登录状态、重启和可选真实 ACP 验收脚本。
- 创建 `.github/workflows/docker-codebuddy-p0.yml`：构建、smoke test，并只推 `:codebuddy-p0`。
- 修改 `.dockerignore`：允许 P0 Dockerfile 复制入口脚本和第三方许可证。
- 修改 `README.md`、`DOCKER.md`：记录 P0 用法、官方登录步骤、删除 volume 的影响及验收标准。

## 非本计划范围

- 不新增 `/setup` 页面或配置 API；它属于 P1。
- 不把 Freemodel key 作为 `CODEBUDDY_API_KEY`/`CODEBUDDY_AUTH_TOKEN`。
- 不覆盖稳定的 `Dockerfile`、`docker-compose.yml`、`:beta` 或 `:latest` 行为。
- 不在 GitHub Actions 中保存或使用真实登录凭据。

---

### 任务 1：入口脚本的进程监督

**文件：**
- 创建：`test/test-codebuddy-p0-entrypoint.sh`
- 创建：`docker/codebuddy-p0-entrypoint.sh`

- [ ] **步骤 1：编写失败的入口脚本测试**

测试创建临时的 `codebuddy` 和 `freemodel-workbuddy-proxy` 假命令，记录参数并保持运行。它验证 gateway 参数、proxy 参数、HOME 目录和收到 TERM 后两个子进程均退出。

```bash
#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENTRYPOINT="$ROOT/docker/codebuddy-p0-entrypoint.sh"
TEMP="$(mktemp -d)"
trap 'rm -rf "$TEMP"' EXIT
mkdir -p "$TEMP/bin" "$TEMP/data" "$TEMP/workspace"

cat >"$TEMP/bin/codebuddy" <<'SH'
#!/bin/bash
printf '%s\n' "$*" >"$P0_CAPTURE/codebuddy.args"
printf '%s\n' "$HOME" >"$P0_CAPTURE/codebuddy.home"
trap 'printf stopped >"$P0_CAPTURE/codebuddy.stopped"; exit 0' TERM INT
while :; do sleep 1; done
SH

cat >"$TEMP/bin/freemodel-workbuddy-proxy" <<'SH'
#!/bin/bash
printf '%s\n' "$*" >"$P0_CAPTURE/proxy.args"
trap 'printf stopped >"$P0_CAPTURE/proxy.stopped"; exit 0' TERM INT
while :; do sleep 1; done
SH
chmod +x "$TEMP/bin/codebuddy" "$TEMP/bin/freemodel-workbuddy-proxy"

PATH="$TEMP/bin:$PATH" \
P0_CAPTURE="$TEMP" \
PROXY_DATA_ROOT="$TEMP/data" \
PROXY_WORKSPACE="$TEMP/workspace" \
CODEBUDDY_GATEWAY_PORT=44741 \
PROXY_PORT=40589 \
"$ENTRYPOINT" &
parent=$!
for _ in $(seq 1 50); do
  [[ -f "$TEMP/codebuddy.args" && -f "$TEMP/proxy.args" ]] && break
  sleep 0.1
done
[[ "$(cat "$TEMP/codebuddy.args")" == "--serve --host 0.0.0.0 --port 44741" ]]
[[ "$(cat "$TEMP/proxy.args")" == "server" ]]
[[ "$(cat "$TEMP/codebuddy.home")" == "$TEMP/data/codebuddy-home" ]]
kill -TERM "$parent"
wait "$parent"
[[ "$(cat "$TEMP/codebuddy.stopped")" == stopped ]]
[[ "$(cat "$TEMP/proxy.stopped")" == stopped ]]
```

- [ ] **步骤 2：运行测试并确认因入口脚本缺失而失败**

运行：

```bash
bash test/test-codebuddy-p0-entrypoint.sh
```

预期：非零退出，并包含 `docker/codebuddy-p0-entrypoint.sh: No such file or directory` 或同义错误。

- [ ] **步骤 3：实现最小入口脚本**

```bash
#!/bin/bash
set -euo pipefail
umask 077

DATA_ROOT="${PROXY_DATA_ROOT:-/data}"
WORKSPACE="${PROXY_WORKSPACE:-/workspace}"
export HOME="${CODEBUDDY_HOME:-$DATA_ROOT/codebuddy-home}"
export PROXY_DEFAULT_PROJECT="${PROXY_DEFAULT_PROJECT:-$WORKSPACE}"
export PROXY_SESSION_STORE="${PROXY_SESSION_STORE:-$DATA_ROOT/sessions.json}"
export PROXY_RUNTIME_DIR="${PROXY_RUNTIME_DIR:-$DATA_ROOT/runtime}"
export WORKBUDDY_EXTERNAL_CWD="${WORKBUDDY_EXTERNAL_CWD:-$WORKSPACE}"
export WORKBUDDY_ACP_URL="${WORKBUDDY_ACP_URL:-http://127.0.0.1:${CODEBUDDY_GATEWAY_PORT:-44741}}"

mkdir -p "$HOME" "$PROXY_RUNTIME_DIR" "$WORKSPACE"
chmod 700 "$HOME" "$PROXY_RUNTIME_DIR"

gateway_pid=
proxy_pid=
stop_children() {
  trap - TERM INT
  [[ -n "$gateway_pid" ]] && kill -TERM "$gateway_pid" 2>/dev/null || true
  [[ -n "$proxy_pid" ]] && kill -TERM "$proxy_pid" 2>/dev/null || true
  [[ -n "$gateway_pid" ]] && wait "$gateway_pid" 2>/dev/null || true
  [[ -n "$proxy_pid" ]] && wait "$proxy_pid" 2>/dev/null || true
}
trap 'stop_children; exit 0' TERM INT

codebuddy --serve --host 0.0.0.0 --port "${CODEBUDDY_GATEWAY_PORT:-44741}" \
  >>"$PROXY_RUNTIME_DIR/codebuddy.log" 2>&1 &
gateway_pid=$!
freemodel-workbuddy-proxy server &
proxy_pid=$!

set +e
wait -n "$gateway_pid" "$proxy_pid"
status=$?
set -e
stop_children
(( status == 0 )) && status=1
exit "$status"
```

- [ ] **步骤 4：运行入口测试验证通过**

运行：

```bash
bash test/test-codebuddy-p0-entrypoint.sh
```

预期：退出码 0，无输出；临时目录由 trap 清理。

- [ ] **步骤 5：做 shell 语法验证**

运行：

```bash
bash -n docker/codebuddy-p0-entrypoint.sh test/test-codebuddy-p0-entrypoint.sh
```

预期：退出码 0。

- [ ] **步骤 6：Commit**

```bash
git add docker/codebuddy-p0-entrypoint.sh test/test-codebuddy-p0-entrypoint.sh
git commit -m "feat: 添加 CodeBuddy P0 进程入口"
```

---

### 任务 2：固定版本的 P0 镜像和许可证

**文件：**
- 创建：`Dockerfile.codebuddy-p0`
- 创建：`THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt`
- 创建：`test/test-codebuddy-p0-files.sh`
- 修改：`.dockerignore`

- [ ] **步骤 1：编写失败的静态镜像约束测试**

```bash
#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKERFILE="$ROOT/Dockerfile.codebuddy-p0"
LICENSE="$ROOT/THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt"
[[ -f "$DOCKERFILE" ]]
[[ -f "$LICENSE" ]]
grep -Fq '@tencent-ai/codebuddy-code@2.133.0' "$DOCKERFILE"
grep -Fq 'FROM node:20-bookworm-slim' "$DOCKERFILE"
grep -Fq 'USER freemodel' "$DOCKERFILE"
grep -Fq 'COPY THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt' "$DOCKERFILE"
grep -Fq 'Permission is hereby granted' "$LICENSE"
! grep -Eq 'fe_[A-Za-z0-9_-]{12,}|CODEBUDDY_(AUTH_TOKEN|API_KEY)=' "$DOCKERFILE" "$LICENSE"
```

- [ ] **步骤 2：运行测试并确认因文件缺失而失败**

运行：

```bash
bash test/test-codebuddy-p0-files.sh
```

预期：非零退出，首个 `[[ -f ... ]]` 失败。

- [ ] **步骤 3：添加官方 MIT 许可证正文**

将 `https://cnb.cool/codebuddy/codebuddy-code/-/git/raw/main/LICENSE.txt` 的完整正文原样保存到 `THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt`，必须包含：

```text
Copyright Copyright 2023 Tencent Cloud

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:
```

并包含余下的 notice 保留条件和 `THE SOFTWARE IS PROVIDED "AS IS"` 免责声明。

- [ ] **步骤 4：创建 P0 Dockerfile**

```dockerfile
# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY tests ./tests
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin freemodel-workbuddy-proxy \
    && cp target/release/freemodel-workbuddy-proxy /usr/local/bin/

FROM node:20-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global --omit=dev --no-audit --no-fund \
      @tencent-ai/codebuddy-code@2.133.0 \
    && codebuddy --version
COPY --from=builder /usr/local/bin/freemodel-workbuddy-proxy /usr/local/bin/
COPY docker/codebuddy-p0-entrypoint.sh /usr/local/bin/
COPY THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt \
  /usr/share/licenses/codebuddy-code/LICENSE.txt
RUN useradd --create-home --uid 10001 freemodel \
    && mkdir -p /app /workspace /data/runtime /data/codebuddy-home \
    && chown -R freemodel:freemodel /app /workspace /data \
    && chmod 755 /usr/local/bin/codebuddy-p0-entrypoint.sh
USER freemodel
WORKDIR /app
ENV PROXY_HOST=0.0.0.0 \
    PROXY_PORT=40589 \
    FREEMODEL_BASE_URL=https://work.freemodel.dev/v1 \
    FREEMODEL_TRANSPORT=workbuddy_acp \
    WORKBUDDY_SIDECAR_MODE=external \
    WORKBUDDY_ACP_URL=http://127.0.0.1:44741 \
    WORKBUDDY_EXTERNAL_CWD=/workspace \
    CODEBUDDY_GATEWAY_AUTH=none \
    PROXY_DEFAULT_PROJECT=/workspace \
    PROXY_SESSION_STORE=/data/sessions.json \
    PROXY_RUNTIME_DIR=/data/runtime
EXPOSE 40589 44741
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS http://127.0.0.1:40589/ready || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/codebuddy-p0-entrypoint.sh"]
```

- [ ] **步骤 5：调整 `.dockerignore`**

删除会阻止新 Dockerfile 构建所需文件进入 context 的宽泛规则：

```text
*.md
LICENSE
```

保留对真实秘密、运行时状态、`test/` 和普通文档目录的排除，并添加：

```text
docs/
README.md
DOCKER.md
LICENSE
```

不要排除 `THIRD_PARTY_LICENSES/` 或 `docker/`。Dockerfile 自身可继续被忽略，因为 `-f Dockerfile.codebuddy-p0` 仍会由 Docker frontend 读取，但为清晰起见把 `Dockerfile` 改为：

```text
Dockerfile
```

不要使用 `Dockerfile*`。

- [ ] **步骤 6：运行静态测试与 secret pattern 检查**

运行：

```bash
bash test/test-codebuddy-p0-files.sh
git grep -nE 'fe_[A-Za-z0-9_-]{12,}|fe_oa_[A-Za-z0-9_-]+' -- \
  Dockerfile.codebuddy-p0 docker THIRD_PARTY_LICENSES test || true
```

预期：测试退出码 0；`git grep` 无真实 key 命中。文档中的通用 `fe_...` 字样不匹配要求长度的 pattern。

- [ ] **步骤 7：Commit**

```bash
git add Dockerfile.codebuddy-p0 .dockerignore \
  THIRD_PARTY_LICENSES/CodeBuddy-Code-MIT.txt test/test-codebuddy-p0-files.sh
git commit -m "feat: 构建 CodeBuddy P0 实验镜像"
```

---

### 任务 3：P0 Compose 和用户验收脚本

**文件：**
- 创建：`docker-compose.codebuddy-p0.yml`
- 创建：`test/test-codebuddy-p0.sh`
- 修改：`test/test-codebuddy-p0-files.sh`

- [ ] **步骤 1：扩展静态测试并确认失败**

在 `test/test-codebuddy-p0-files.sh` 追加：

```bash
COMPOSE="$ROOT/docker-compose.codebuddy-p0.yml"
[[ -f "$COMPOSE" ]]
grep -Fq '127.0.0.1:40589:40589' "$COMPOSE"
grep -Fq '127.0.0.1:44741:44741' "$COMPOSE"
grep -Fq 'codebuddy-p0-data:/data' "$COMPOSE"
! grep -Fq 'network_mode: host' "$COMPOSE"
! grep -Eq 'FREEMODEL_API_KEY:|CODEBUDDY_AUTH_TOKEN:|CODEBUDDY_API_KEY:' "$COMPOSE"
```

运行：

```bash
bash test/test-codebuddy-p0-files.sh
```

预期：因 Compose 缺失而失败。

- [ ] **步骤 2：创建隔离 Compose**

```yaml
services:
  freemodel-proxy:
    image: ghcr.io/${IMAGE_REPO:-murasamecyan/ciallo_freemodel_proxy}:codebuddy-p0
    build:
      context: .
      dockerfile: Dockerfile.codebuddy-p0
    container_name: ciallo-codebuddy-p0
    restart: unless-stopped
    ports:
      - "127.0.0.1:40589:40589"
      - "127.0.0.1:44741:44741"
    environment:
      PROXY_API_KEY: ${PROXY_API_KEY:-}
    volumes:
      - codebuddy-p0-data:/data
      - ./workspace:/workspace

volumes:
  codebuddy-p0-data:
```

- [ ] **步骤 3：创建用户验收脚本**

脚本只检查公开健康状态和可选真实请求，不接收上游 key：

```bash
#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE=(docker compose -f "$ROOT/docker-compose.codebuddy-p0.yml")

"${COMPOSE[@]}" up -d --build
for _ in $(seq 1 120); do
  curl -fsS http://127.0.0.1:40589/ready >/dev/null 2>&1 && break
  sleep 1
done
curl -fsS http://127.0.0.1:40589/health >/dev/null
curl -fsS -H 'x-codebuddy-request: 1' \
  http://127.0.0.1:44741/api/v1/health >/dev/null

echo 'P0 gateway 已启动。首次登录可尝试打开 http://127.0.0.1:44741，'
echo '或运行：docker exec -it ciallo-codebuddy-p0 codebuddy'
echo '在官方 CLI 输入 /login。登录完成后以 RUN_LIVE_ACP_TEST=1 重跑本脚本。'

if [[ "${RUN_LIVE_ACP_TEST:-0}" == 1 ]]; then
  response=$(curl -fsS http://127.0.0.1:40589/v1/chat/completions \
    -H "Authorization: Bearer ${PROXY_API_KEY:-local-proxy}" \
    -H 'Content-Type: application/json' \
    -d '{"model":"gpt-5.6-sol","messages":[{"role":"user","content":"只回复 OK"}]}')
  python - "$response" <<'PY'
import json, sys
body = json.loads(sys.argv[1])
assert body["choices"][0]["message"]["content"].strip(), body
print("真实 WorkBuddy ACP 请求通过")
PY
fi
```

- [ ] **步骤 4：验证 shell 和 Compose 静态约束**

运行：

```bash
bash -n test/test-codebuddy-p0.sh
bash test/test-codebuddy-p0-files.sh
```

若 Docker 可用，额外运行：

```bash
docker compose -f docker-compose.codebuddy-p0.yml config --quiet
```

预期：所有可运行命令退出码 0；无 Docker 环境明确记录该项未执行。

- [ ] **步骤 5：Commit**

```bash
git add docker-compose.codebuddy-p0.yml \
  test/test-codebuddy-p0.sh test/test-codebuddy-p0-files.sh
git commit -m "test: 添加 CodeBuddy P0 验收入口"
```

---

### 任务 4：P0 专用 GitHub Actions 发布

**文件：**
- 创建：`.github/workflows/docker-codebuddy-p0.yml`
- 修改：`test/test-codebuddy-p0-files.sh`

- [ ] **步骤 1：扩展 workflow 静态约束并确认失败**

在 `test/test-codebuddy-p0-files.sh` 追加：

```bash
WORKFLOW="$ROOT/.github/workflows/docker-codebuddy-p0.yml"
[[ -f "$WORKFLOW" ]]
grep -Fq 'Dockerfile.codebuddy-p0' "$WORKFLOW"
grep -Fq 'codebuddy-p0' "$WORKFLOW"
! grep -Eq 'value=(beta|latest)' "$WORKFLOW"
! grep -Eq 'FREEMODEL_API_KEY|CODEBUDDY_AUTH_TOKEN|CODEBUDDY_API_KEY' "$WORKFLOW"
```

运行：

```bash
bash test/test-codebuddy-p0-files.sh
```

预期：因 workflow 缺失而失败。

- [ ] **步骤 2：创建 P0 workflow**

Workflow 仅在手动触发或 P0 相关文件推到 `beta` 时运行，执行 Rust 测试和两个 shell 测试，然后用 Buildx 构建单一 `linux/amd64` 镜像到本地 Docker，smoke test 固定版本，最后仅推实验标签。

```yaml
name: Build CodeBuddy P0 Image
on:
  workflow_dispatch:
  push:
    branches: [beta]
    paths:
      - Dockerfile.codebuddy-p0
      - docker/codebuddy-p0-entrypoint.sh
      - docker-compose.codebuddy-p0.yml
      - THIRD_PARTY_LICENSES/**
      - src/**
      - tests/**
      - test/test-codebuddy-p0*.sh
      - Cargo.toml
      - Cargo.lock
      - build.rs
      - .github/workflows/docker-codebuddy-p0.yml

jobs:
  build:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write
    steps:
      - uses: actions/checkout@v4
      - name: Run Rust tests
        run: cargo test --all-targets
      - name: Run P0 shell tests
        run: |
          bash test/test-codebuddy-p0-entrypoint.sh
          bash test/test-codebuddy-p0-files.sh
      - name: Compute lowercase image name
        id: img
        run: echo "name=ghcr.io/${GITHUB_REPOSITORY,,}" >> "$GITHUB_OUTPUT"
      - uses: docker/setup-buildx-action@v3
      - name: Build P0 image locally
        uses: docker/build-push-action@v5
        with:
          context: .
          file: Dockerfile.codebuddy-p0
          platforms: linux/amd64
          load: true
          push: false
          tags: ${{ steps.img.outputs.name }}:codebuddy-p0
          cache-from: type=gha,scope=codebuddy-p0
          cache-to: type=gha,mode=max,scope=codebuddy-p0
      - name: Smoke test packaged CLI
        run: |
          docker run --rm --entrypoint codebuddy \
            "${{ steps.img.outputs.name }}:codebuddy-p0" --version \
            | grep -F '2.133.0'
          test "$(docker inspect --format '{{.Config.User}}' \
            "${{ steps.img.outputs.name }}:codebuddy-p0")" = freemodel
      - name: Log in to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}
      - name: Push P0 tag only
        run: docker push "${{ steps.img.outputs.name }}:codebuddy-p0"
```

- [ ] **步骤 3：验证 YAML 和静态约束**

运行：

```bash
bash test/test-codebuddy-p0-files.sh
python - <<'PY'
from pathlib import Path
import yaml
p = Path('.github/workflows/docker-codebuddy-p0.yml')
yaml.safe_load(p.read_text(encoding='utf-8'))
print('workflow yaml: ok')
PY
```

如果环境没有 PyYAML，使用 Ruby 标准库替代：

```bash
ruby -e "require 'yaml'; YAML.load_file('.github/workflows/docker-codebuddy-p0.yml'); puts 'workflow yaml: ok'"
```

预期：解析成功，shell 测试退出码 0。

- [ ] **步骤 4：Commit**

```bash
git add .github/workflows/docker-codebuddy-p0.yml test/test-codebuddy-p0-files.sh
git commit -m "ci: 发布 CodeBuddy P0 实验镜像"
```

---

### 任务 5：P0 用户文档

**文件：**
- 修改：`README.md`
- 修改：`DOCKER.md`

- [ ] **步骤 1：添加 README 实验入口**

在 Docker 快速开始前添加“CodeBuddy P0 实验镜像”小节，必须明确：

```markdown
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

不要执行 `docker compose ... down -v`，除非你明确要删除 `/data` volume 和登录态。
```

- [ ] **步骤 2：在 DOCKER.md 添加故障矩阵**

表格至少包含：

| 现象 | 含义 | 操作 |
| --- | --- | --- |
| `44741` health 不通 | gateway 未启动 | 查看 `/data/runtime/codebuddy.log`，不得公开整份可能含敏感上下文的日志 |
| 模型请求 401/403 | gateway 未登录或账号无权 | 运行官方 `/login`，不要设置 `CODEBUDDY_API_KEY=fe_...` |
| 登录回调打不开 | P0 登录链路不成立 | 记录浏览器 URL/错误码但删去 token，停止 P1 |
| 重启后再次要求登录 | 登录态未正确落在 `/data` | 不发布 P1，先定位官方实际配置目录 |
| quota/max_instances | 官方账号限制 | 等待/清理官方实例，不修改代理本地并发冒充解决 |

同时写明 `:codebuddy-p0` 仅是实验标签，稳定版继续采用宿主 external gateway。

- [ ] **步骤 3：检查文档与真实配置一致**

运行：

```bash
git diff --check
git grep -n 'codebuddy-p0' -- README.md DOCKER.md \
  docker-compose.codebuddy-p0.yml .github/workflows/docker-codebuddy-p0.yml
```

预期：diff check 退出码 0；所有命令、标签和容器名一致。

- [ ] **步骤 4：Commit**

```bash
git add README.md DOCKER.md
git commit -m "docs: 添加 CodeBuddy P0 登录验收指南"
```

---

### 任务 6：完整验证、密钥扫描和发布实验标签

**文件：**
- 验证全部 P0 变更；无新的生产文件

- [ ] **步骤 1：运行全部本机可用检查**

```bash
bash -n docker/codebuddy-p0-entrypoint.sh \
  test/test-codebuddy-p0-entrypoint.sh \
  test/test-codebuddy-p0-files.sh \
  test/test-codebuddy-p0.sh
bash test/test-codebuddy-p0-entrypoint.sh
bash test/test-codebuddy-p0-files.sh
git diff --check
git status --short
```

若本机有 Cargo，再运行：

```bash
cargo test --all-targets
```

若本机有 Docker，再运行：

```bash
docker build -f Dockerfile.codebuddy-p0 -t ciallo/codebuddy-p0:test .
docker run --rm --entrypoint codebuddy ciallo/codebuddy-p0:test --version
docker compose -f docker-compose.codebuddy-p0.yml config --quiet
```

预期：可用检查全部成功。缺失工具必须如实记录，不能声称对应检查通过。

- [ ] **步骤 2：扫描工作树中的 key**

```bash
if git grep -nEI 'fe(_oa)?_[A-Za-z0-9_-]{16,}|(CODEBUDDY_AUTH_TOKEN|CODEBUDDY_API_KEY)[=:][^[:space:]]+' -- . \
  ':!docs/superpowers/specs/*' ':!docs/superpowers/plans/*'; then
  echo '发现疑似真实密钥，停止发布' >&2
  exit 1
fi
```

预期：无命中。测试中的变量名或明确空值不构成秘密，但任何非空疑似值必须人工核查。

- [ ] **步骤 3：扫描所有本地可达提交**

```bash
for commit in $(git rev-list --all); do
  git grep -nEI 'fe(_oa)?_[A-Za-z0-9_-]{16,}' "$commit" -- . && exit 1 || true
done
```

预期：无命中；如命中，停止推送并先完成凭据轮换与经用户授权的历史清理。

- [ ] **步骤 4：推送 `beta` 触发 P0 workflow**

```bash
git push origin beta
```

预期：只触发新 workflow 推送 `:codebuddy-p0`；既有 workflow 可能重建稳定镜像，但 P0 Dockerfile 不改变稳定镜像内容。

- [ ] **步骤 5：等待并核验 GitHub Actions**

```bash
gh run list --branch beta --limit 5
gh run watch <P0_RUN_ID> --exit-status
```

预期：Rust 测试、shell 测试、Docker build、`codebuddy --version`、非 root inspect 和 GHCR push 全部成功。失败时保留任务为进行中，读取真实日志后修复。

- [ ] **步骤 6：匿名验证 GHCR manifest**

```bash
curl -fsSI \
  https://ghcr.io/v2/murasamecyan/ciallo_freemodel_proxy/manifests/codebuddy-p0 \
  -H 'Accept: application/vnd.oci.image.manifest.v1+json'
```

预期：HTTP 200。若 GHCR 需要匿名 token，先按 registry challenge 获取只读 token，再请求 manifest；不得把 GitHub token写入日志。

- [ ] **步骤 7：向用户交付 P0 试用命令**

只在 Actions 和 manifest 验证后给出：

```bash
docker pull ghcr.io/murasamecyan/ciallo_freemodel_proxy:codebuddy-p0
docker compose -f docker-compose.codebuddy-p0.yml up -d
docker exec -it ciallo-codebuddy-p0 codebuddy
```

明确要求用户在 `/login` 后先运行真实请求，再重启容器重复运行。用户反馈登录、真实 ACP、重启持久化三项都通过后，才创建并执行 P1 Web Setup 计划。
