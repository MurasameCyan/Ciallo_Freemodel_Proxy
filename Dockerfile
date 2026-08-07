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
    && mkdir -p /workspace /data/runtime \
    && chown -R freemodel:freemodel /workspace /data

USER freemodel
# 工作目录决定 config.json 的位置（config.rs 用 project_root 拼），放在 /data
# 这个持久卷上，`key set` 存的 key 才不会随容器重建丢失。
# 刻意不用 /workspace：那是用户挂载的目录，而 config.json 的优先级高于环境变量，
# 用户目录里若带着 config.json 就会静默覆盖镜像配置。
WORKDIR /data
ENV HOME=/home/freemodel

EXPOSE 40589

# 默认走 cc.freemodel.dev：Anthropic Messages 协议，鉴权只需
# Bearer {FREEMODEL_API_KEY}，容器内不需要任何登录态或额外进程。
# 想改用 work.freemodel.dev 需同时覆盖 FREEMODEL_TRANSPORT=workbuddy_acp
# 与 WORKBUDDY_EXTERNAL_CWD（宿主机可见的真实路径，无法从容器推导）。
ENV PROXY_HOST=0.0.0.0 \
    PROXY_PORT=40589 \
    FREEMODEL_BASE_URL=https://cc.freemodel.dev/v1 \
    FREEMODEL_TRANSPORT=cc_anthropic \
    WORKBUDDY_SIDECAR_MODE=external \
    WORKBUDDY_ACP_URL=http://127.0.0.1:44741 \
    PROXY_DEFAULT_PROJECT=/workspace \
    PROXY_SESSION_STORE=/data/sessions.json \
    PROXY_RUNTIME_DIR=/data/runtime

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:40589/ready || exit 1

ENTRYPOINT ["freemodel-workbuddy-proxy"]
CMD ["server"]
