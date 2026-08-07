# syntax=docker/dockerfile:1

# ── build ────────────────────────────────────────────────────────────────
# rust:1-slim 跟随最新稳定版，满足 Cargo.toml 的 edition 2024 / rust-version 1.97。
FROM rust:1-slim-bookworm AS builder

WORKDIR /build

# 依赖用 rustls（见 Cargo.toml reqwest features），不需要 openssl 开发库。
# tests/ 必须复制：Cargo.toml 声明了 [[bin]] fake-codebuddy 指向
# tests/fixtures/fake_codebuddy.rs，cargo 会校验该路径存在，缺失则构建失败。
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY tests ./tests

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin freemodel-workbuddy-proxy \
    && cp target/release/freemodel-workbuddy-proxy /usr/local/bin/

# ── runtime ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/freemodel-workbuddy-proxy /usr/local/bin/

# /workspace 必须真实存在：config.rs 会 canonicalize PROXY_DEFAULT_PROJECT，
# 目录不存在则启动直接报错退出。
# /data 存放代理会话与运行时文件。
# 用户名不能叫 proxy：debian-slim 自带同名系统账户（uid 13），useradd 会失败。
RUN useradd --create-home --uid 10001 freemodel \
    && mkdir -p /app /workspace /data/runtime \
    && chown -R freemodel:freemodel /app /workspace /data

USER freemodel
# 工作目录刻意不用 /workspace：config.rs 中 config.json 的优先级高于环境变量，
# 而 project_root 会回落到工作目录。若用户挂载的 /workspace 里带着 config.json，
# 就会静默覆盖镜像的环境变量配置。/app 保持为空即可避免。
WORKDIR /app
ENV HOME=/home/freemodel

EXPOSE 40589

# work.freemodel.dev 必须经官方 WorkBuddy 客户端已认证的 ACP gateway 访问。
# 容器通过 WORKBUDDY_ACP_URL 连接宿主机 gateway，不复制或模拟私有认证。
# WORKBUDDY_EXTERNAL_CWD 无法从容器推导，运行时必须传入宿主机可见的真实路径。
ENV PROXY_HOST=0.0.0.0 \
    PROXY_PORT=40589 \
    FREEMODEL_BASE_URL=https://work.freemodel.dev/v1 \
    FREEMODEL_TRANSPORT=workbuddy_acp \
    WORKBUDDY_SIDECAR_MODE=external \
    WORKBUDDY_ACP_URL=http://127.0.0.1:44741 \
    PROXY_DEFAULT_PROJECT=/workspace \
    PROXY_SESSION_STORE=/data/sessions.json \
    PROXY_RUNTIME_DIR=/data/runtime

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:40589/ready || exit 1

ENTRYPOINT ["freemodel-workbuddy-proxy"]
CMD ["server"]
