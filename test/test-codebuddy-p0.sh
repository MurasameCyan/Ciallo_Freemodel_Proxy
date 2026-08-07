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
