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
