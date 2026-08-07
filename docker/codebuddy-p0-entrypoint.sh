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

# 挂载边界守卫：/workspace 通常是宿主机 bind mount，其属主由宿主机决定，
# 常与容器内 uid 10001 不匹配。不早退的话失败会推迟到 CodeBuddy 写文件时，
# 表现为难以归因的报错，直接污染 P0「登录能否在纯 Docker 完成」的结论。
ensure_writable_dir() {
  local dir=$1 hint=$2
  mkdir -p "$dir" 2>/dev/null || true
  if [[ ! -d "$dir" || ! -w "$dir" ]]; then
    printf '致命错误：%s 不可写（容器内 uid=%s）。%s\n' "$dir" "$(id -u)" "$hint" >&2
    exit 1
  fi
}

ensure_writable_dir "$HOME" '删除并重建 codebuddy-p0-data volume 后重启容器。'
ensure_writable_dir "$PROXY_RUNTIME_DIR" '删除并重建 codebuddy-p0-data volume 后重启容器。'
ensure_writable_dir "$WORKSPACE" \
  '在宿主机执行 mkdir -p ./workspace && sudo chown 10001:10001 ./workspace 后重启容器。'
chmod 700 "$HOME" "$PROXY_RUNTIME_DIR"

gateway_pid=
proxy_pid=
startup_complete=0
shutdown_requested=0
stop_children() {
  trap - TERM INT
  [[ -n "$gateway_pid" ]] && kill -TERM "$gateway_pid" 2>/dev/null || true
  [[ -n "$proxy_pid" ]] && kill -TERM "$proxy_pid" 2>/dev/null || true
  [[ -n "$gateway_pid" ]] && wait "$gateway_pid" 2>/dev/null || true
  [[ -n "$proxy_pid" ]] && wait "$proxy_pid" 2>/dev/null || true
}
handle_signal() {
  shutdown_requested=1
  if (( startup_complete )); then
    stop_children
    exit 0
  fi
}
trap handle_signal TERM INT

codebuddy --serve --host 0.0.0.0 --port "${CODEBUDDY_GATEWAY_PORT:-44741}" \
  >>"$PROXY_RUNTIME_DIR/codebuddy.log" 2>&1 &
gateway_pid=$!
if (( shutdown_requested )); then
  stop_children
  exit 0
fi
freemodel-workbuddy-proxy server &
proxy_pid=$!
startup_complete=1
if (( shutdown_requested )); then
  stop_children
  exit 0
fi

set +e
wait -n "$gateway_pid" "$proxy_pid"
status=$?
set -e
stop_children
(( status == 0 )) && status=1
exit "$status"
