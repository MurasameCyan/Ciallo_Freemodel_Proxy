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
