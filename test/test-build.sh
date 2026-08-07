#!/bin/bash
# 本地构建测试脚本：验证 Docker 镜像和外部 WorkBuddy ACP 配置。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTAINER=freemodel-proxy-test

: "${WORKBUDDY_EXTERNAL_CWD:?错误：请设置宿主机 WorkBuddy 可访问的真实工作目录}"

cleanup() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

if ! curl -fsS -H 'x-codebuddy-request: 1' \
    http://127.0.0.1:44741/api/v1/health >/dev/null; then
    echo "错误：宿主机 WorkBuddy ACP gateway 未在 127.0.0.1:44741 就绪。" >&2
    echo "请先启动并登录官方 WorkBuddy；若端口不同，修改本脚本或手动测试。" >&2
    exit 1
fi

echo "构建 Docker 镜像……"
docker build -t freemodel-proxy:test "$ROOT"

echo "启动 host-network 测试容器……"
cleanup
docker run -d \
  --name "$CONTAINER" \
  --network host \
  -e PROXY_HOST=127.0.0.1 \
  -e FREEMODEL_BASE_URL=https://work.freemodel.dev/v1 \
  -e FREEMODEL_TRANSPORT=workbuddy_acp \
  -e WORKBUDDY_SIDECAR_MODE=external \
  -e WORKBUDDY_ACP_URL=http://127.0.0.1:44741 \
  -e WORKBUDDY_EXTERNAL_CWD="$WORKBUDDY_EXTERNAL_CWD" \
  freemodel-proxy:test >/dev/null

for _ in $(seq 1 30); do
    if health=$(curl -fsS http://127.0.0.1:40589/health 2>/dev/null) \
      && curl -fsS http://127.0.0.1:40589/ready >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

if [[ -z "${health:-}" ]]; then
    docker logs "$CONTAINER" >&2
    echo "错误：代理或 WorkBuddy ACP gateway 未就绪。" >&2
    exit 1
fi

python - "$health" <<'PY'
import json, sys
health = json.loads(sys.argv[1])
assert health["status"] == "ok", health
assert health["transport"] == "workbuddy_acp", health
assert health["upstream_mode"] == "official_external_acp", health
print("健康检查通过：workbuddy_acp / official_external_acp")
PY

# 默认不发送真实模型请求，以免测试脚本未经确认使用上游资源。
# 需要端到端验证时显式设置 RUN_LIVE_ACP_TEST=1。
if [[ "${RUN_LIVE_ACP_TEST:-0}" == "1" ]]; then
    response=$(curl -fsS http://127.0.0.1:40589/v1/chat/completions \
      -H 'Authorization: Bearer local-proxy' \
      -H 'Content-Type: application/json' \
      -d '{"model":"gpt-5.6-sol","messages":[{"role":"user","content":"只回复 OK"}]}')
    python - "$response" <<'PY'
import json, sys
response = json.loads(sys.argv[1])
text = response["choices"][0]["message"]["content"]
assert text.strip(), response
print("真实 WorkBuddy ACP 对话通过")
PY
fi

echo "Docker 镜像验证完成。"
