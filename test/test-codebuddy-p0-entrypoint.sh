#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENTRYPOINT="${P0_ENTRYPOINT:-$ROOT/docker/codebuddy-p0-entrypoint.sh}"
TEMP="$(mktemp -d)"
CASE_DIR=
parent=
parent_reaped=1
parent_status=

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

wait_for_file() {
  local file=$1
  local description=$2
  local i
  for i in $(seq 1 100); do
    [[ -f "$file" ]] && return 0
    sleep 0.05
  done
  fail "timed out waiting for $description"
}

wait_for_process_exit() {
  local pid=$1
  local description=$2
  local i
  for i in $(seq 1 100); do
    ! kill -0 "$pid" 2>/dev/null && return 0
    sleep 0.05
  done
  fail "timed out waiting for $description (pid $pid) to exit"
}

wait_for_parent() {
  local completed_pid status timeout_pid
  (( parent_reaped == 0 )) || return 0

  sleep 5 &
  timeout_pid=$!
  set +e
  wait -n -p completed_pid "$parent" "$timeout_pid"
  status=$?
  set -e

  if [[ "$completed_pid" == "$parent" ]]; then
    parent_status=$status
    parent_reaped=1
    kill -TERM "$timeout_pid" 2>/dev/null || true
    wait "$timeout_pid" 2>/dev/null || true
    return 0
  fi

  fail "timed out waiting for entrypoint in ${CASE_DIR##*/} (pid $parent) to exit"
}

stop_case_processes() {
  local pid_file pid

  if [[ -n "$parent" && $parent_reaped -eq 0 ]]; then
    kill -TERM "$parent" 2>/dev/null || true
    for _ in $(seq 1 40); do
      ! kill -0 "$parent" 2>/dev/null && break
      sleep 0.05
    done
    if kill -0 "$parent" 2>/dev/null; then
      kill -KILL "$parent" 2>/dev/null || true
    fi
    if wait "$parent" 2>/dev/null; then
      parent_status=0
    else
      parent_status=$?
    fi
    parent_reaped=1
  fi

  [[ -n "$CASE_DIR" ]] || return 0
  for pid_file in "$CASE_DIR/codebuddy.pid" "$CASE_DIR/proxy.pid"; do
    [[ -f "$pid_file" ]] || continue
    pid=$(<"$pid_file")
    kill -TERM "$pid" 2>/dev/null || true
  done
  for pid_file in "$CASE_DIR/codebuddy.pid" "$CASE_DIR/proxy.pid"; do
    [[ -f "$pid_file" ]] || continue
    pid=$(<"$pid_file")
    for _ in $(seq 1 40); do
      ! kill -0 "$pid" 2>/dev/null && break
      sleep 0.05
    done
    if kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
      for _ in $(seq 1 40); do
        ! kill -0 "$pid" 2>/dev/null && break
        sleep 0.05
      done
    fi
  done
}

cleanup() {
  stop_case_processes
  rm -rf "$TEMP"
}
trap cleanup EXIT

mkdir -p "$TEMP/bin"
cat >"$TEMP/bin/codebuddy" <<'SH'
#!/bin/bash
printf '%s\n' "$$" >"$P0_CAPTURE/codebuddy.pid"
printf '%s\n' "$*" >"$P0_CAPTURE/codebuddy.args"
printf '%s\n' "$HOME" >"$P0_CAPTURE/codebuddy.home"
trap 'printf stopped >"$P0_CAPTURE/codebuddy.stopped"; exit 0' TERM INT
printf 'ready\n' >"$P0_CAPTURE/gateway.ready"
while [[ ! -f "$P0_CAPTURE/codebuddy.exit" ]]; do sleep 0.05; done
exit "${P0_CODEBUDDY_EXIT_STATUS:-7}"
SH

cat >"$TEMP/bin/freemodel-workbuddy-proxy" <<'SH'
#!/bin/bash
printf '%s\n' "$$" >"$P0_CAPTURE/proxy.pid"
printf '%s\n' "$*" >"$P0_CAPTURE/proxy.args"
trap 'printf stopped >"$P0_CAPTURE/proxy.stopped"; exit 0' TERM INT
printf 'ready\n' >"$P0_CAPTURE/proxy.ready"
while [[ ! -f "$P0_CAPTURE/proxy.exit" ]]; do sleep 0.05; done
exit "${P0_PROXY_EXIT_STATUS:-9}"
SH
chmod +x "$TEMP/bin/codebuddy" "$TEMP/bin/freemodel-workbuddy-proxy"

cat >"$TEMP/inject-startup-signal.sh" <<'SH'
kill() {
  printf '%s\n' "${!#}" >>"$P0_CAPTURE/kill.calls"
  builtin kill "$@"
}
p0_inject_startup_signal() {
  local i
  if [[ "$BASH_COMMAND" == "$P0_SIGNAL_BEFORE=\$!" ]]; then
    trap - DEBUG
    printf 'hit\n' >"$P0_CAPTURE/signal.injected"
    builtin kill -"$P0_STARTUP_SIGNAL" "$$"
    for i in $(seq 1 100); do
      [[ -f "$P0_CAPTURE/$P0_SIGNAL_READY_FILE" ]] && return 0
      sleep 0.01
    done
  fi
}
trap p0_inject_startup_signal DEBUG
SH

begin_case() {
  local name=$1
  stop_case_processes
  CASE_DIR="$TEMP/$name"
  mkdir -p "$CASE_DIR/data" "$CASE_DIR/workspace"
  parent=
  parent_reaped=1
  parent_status=
}

start_entrypoint() {
  PATH="$TEMP/bin:$PATH" \
  P0_CAPTURE="$CASE_DIR" \
  PROXY_DATA_ROOT="$CASE_DIR/data" \
  PROXY_WORKSPACE="$CASE_DIR/workspace" \
  CODEBUDDY_GATEWAY_PORT=44741 \
  PROXY_PORT=40589 \
  "$ENTRYPOINT" &
  parent=$!
  parent_reaped=0
}

start_entrypoint_with_signal() {
  local before_assignment=$1
  local signal=$2
  local ready_file=$3
  if [[ "$signal" == INT ]]; then
    set -m
  fi
  PATH="$TEMP/bin:$PATH" \
  BASH_ENV="$TEMP/inject-startup-signal.sh" \
  P0_SIGNAL_BEFORE="$before_assignment" \
  P0_STARTUP_SIGNAL="$signal" \
  P0_SIGNAL_READY_FILE="$ready_file" \
  P0_CAPTURE="$CASE_DIR" \
  PROXY_DATA_ROOT="$CASE_DIR/data" \
  PROXY_WORKSPACE="$CASE_DIR/workspace" \
  CODEBUDDY_GATEWAY_PORT=44741 \
  PROXY_PORT=40589 \
  "$ENTRYPOINT" &
  parent=$!
  parent_reaped=0
  if [[ "$signal" == INT ]]; then
    set +m
  fi
}

begin_case normal-term
start_entrypoint
wait_for_file "$CASE_DIR/codebuddy.args" 'gateway arguments'
wait_for_file "$CASE_DIR/proxy.args" 'proxy arguments'
[[ "$(<"$CASE_DIR/codebuddy.args")" == "--serve --host 0.0.0.0 --port 44741" ]] || fail 'unexpected gateway arguments'
[[ "$(<"$CASE_DIR/proxy.args")" == "server" ]] || fail 'unexpected proxy arguments'
[[ "$(<"$CASE_DIR/codebuddy.home")" == "$CASE_DIR/data/codebuddy-home" ]] || fail 'unexpected gateway HOME'
kill -TERM "$parent"
wait_for_parent
[[ $parent_status -eq 0 ]] || fail "TERM exit status was $parent_status"
wait_for_file "$CASE_DIR/codebuddy.stopped" 'gateway TERM shutdown'
wait_for_file "$CASE_DIR/proxy.stopped" 'proxy TERM shutdown'

begin_case startup-gateway-term
start_entrypoint_with_signal gateway_pid TERM gateway.ready
wait_for_parent
[[ $parent_status -eq 0 ]] || fail "startup TERM exit status was $parent_status"
wait_for_file "$CASE_DIR/signal.injected" 'gateway startup TERM injection'
wait_for_file "$CASE_DIR/kill.calls" 'gateway cleanup after startup TERM'
startup_pid=$(<"$CASE_DIR/kill.calls")
[[ -n "$startup_pid" ]] || fail 'gateway startup TERM did not target a child'
wait_for_process_exit "$startup_pid" 'gateway after startup TERM'

begin_case startup-proxy-int
start_entrypoint_with_signal proxy_pid INT proxy.ready
wait_for_parent
[[ $parent_status -eq 0 ]] || fail "startup INT exit status was $parent_status"
wait_for_file "$CASE_DIR/signal.injected" 'proxy startup INT injection'
wait_for_file "$CASE_DIR/kill.calls" 'child cleanup after startup INT'
mapfile -t startup_pids <"$CASE_DIR/kill.calls"
[[ ${#startup_pids[@]} -eq 2 ]] || fail "startup INT cleaned ${#startup_pids[@]} children instead of 2"
wait_for_process_exit "${startup_pids[0]}" 'gateway after startup INT'
wait_for_process_exit "${startup_pids[1]}" 'proxy after startup INT'

begin_case gateway-exits
start_entrypoint
wait_for_file "$CASE_DIR/codebuddy.pid" 'gateway startup'
wait_for_file "$CASE_DIR/proxy.pid" 'proxy startup'
: >"$CASE_DIR/codebuddy.exit"
wait_for_parent
[[ $parent_status -ne 0 ]] || fail 'entrypoint succeeded after gateway exited'
wait_for_file "$CASE_DIR/proxy.stopped" 'proxy shutdown after gateway exit'
wait_for_process_exit "$(<"$CASE_DIR/codebuddy.pid")" 'exited gateway'
wait_for_process_exit "$(<"$CASE_DIR/proxy.pid")" 'proxy after gateway exit'

begin_case proxy-exits
start_entrypoint
wait_for_file "$CASE_DIR/codebuddy.pid" 'gateway startup'
wait_for_file "$CASE_DIR/proxy.pid" 'proxy startup'
: >"$CASE_DIR/proxy.exit"
wait_for_parent
[[ $parent_status -ne 0 ]] || fail 'entrypoint succeeded after proxy exited'
wait_for_file "$CASE_DIR/codebuddy.stopped" 'gateway shutdown after proxy exit'
wait_for_process_exit "$(<"$CASE_DIR/codebuddy.pid")" 'gateway after proxy exit'
wait_for_process_exit "$(<"$CASE_DIR/proxy.pid")" 'exited proxy'

begin_case unwritable-workspace
chmod 500 "$CASE_DIR/workspace"
if [[ "$(id -u)" == 0 || -w "$CASE_DIR/workspace" ]]; then
  printf 'SKIP: 当前环境无法造出不可写目录（root 或 chmod 无效）\n' >&2
  chmod 700 "$CASE_DIR/workspace"
else
  set +e
  PATH="$TEMP/bin:$PATH" \
  P0_CAPTURE="$CASE_DIR" \
  PROXY_DATA_ROOT="$CASE_DIR/data" \
  PROXY_WORKSPACE="$CASE_DIR/workspace" \
  CODEBUDDY_GATEWAY_PORT=44741 \
  PROXY_PORT=40589 \
  "$ENTRYPOINT" >"$CASE_DIR/guard.out" 2>"$CASE_DIR/guard.err"
  guard_status=$?
  set -e
  chmod 700 "$CASE_DIR/workspace"
  (( guard_status != 0 )) || fail '不可写 workspace 未导致入口脚本退出'
  grep -Fq "$CASE_DIR/workspace" "$CASE_DIR/guard.err" || fail '守卫未指出不可写目录'
  grep -Fq 'chown 10001:10001' "$CASE_DIR/guard.err" || fail '守卫未给出可执行的修复提示'
  [[ ! -f "$CASE_DIR/codebuddy.pid" ]] || fail '守卫失败前已启动 gateway'
  [[ ! -f "$CASE_DIR/proxy.pid" ]] || fail '守卫失败前已启动 proxy'
fi

stop_case_processes
CASE_DIR=
