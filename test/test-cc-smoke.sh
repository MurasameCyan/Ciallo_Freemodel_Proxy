#!/bin/bash
# 验证默认镜像开箱就是 cc.freemodel.dev 路径：不需要登录、不需要宿主机 gateway。
# 不发任何上游请求，因此不消耗额度、也不需要真实 key。
# 用法：bash test/test-cc-smoke.sh <image[:tag]>
set -euo pipefail

IMAGE="${1:?用法: test-cc-smoke.sh <image[:tag]>}"
CONTAINER=freemodel-proxy-cc-smoke
PORT=40589
KEY=smoke-proxy-key

cleanup() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

docker run -d \
  --name "$CONTAINER" \
  -p "127.0.0.1:$PORT:40589" \
  -e PROXY_API_KEY="$KEY" \
  "$IMAGE" >/dev/null

health=""
for _ in $(seq 1 30); do
    if health=$(curl -fsS "http://127.0.0.1:$PORT/health" 2>/dev/null); then
        break
    fi
    health=""
    sleep 1
done

if [[ -z "$health" ]]; then
    docker logs "$CONTAINER" >&2
    echo "FAIL: 代理未就绪" >&2
    exit 1
fi

python3 - "$health" <<'PY'
import json, sys
health = json.loads(sys.argv[1])
assert health["status"] == "ok", health
# 镜像默认必须落在 cc 路径上，否则用户 pull 下来还得自己配。
assert health["transport"] == "cc_anthropic", health
assert health["upstream_mode"] == "direct_anthropic", health
print("OK: health 报告 cc_anthropic / direct_anthropic")
PY

# /ready 不依赖宿主机 gateway，cc 路径下应直接 ready。
curl -fsS "http://127.0.0.1:$PORT/ready" >/dev/null
echo "OK: /ready 无需外部 gateway"

# 没有 key 时必须 401：镜像绑 0.0.0.0，鉴权失效等于把额度公开。
code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/v1/models")
if [[ "$code" != "401" ]]; then
    echo "FAIL: 未鉴权的 /v1/models 返回 $code，应为 401" >&2
    exit 1
fi
echo "OK: PROXY_API_KEY 生效，未鉴权请求被拒"

models=$(curl -fsS -H "Authorization: Bearer $KEY" "http://127.0.0.1:$PORT/v1/models")
python3 - "$models" <<'PY'
import json, sys
ids = [m["id"] for m in json.loads(sys.argv[1])["data"]]
# cc 只有这三个真实后端；报 gpt-* 会让客户端选到必被上游改换的名字。
assert ids == [
    "claude-opus-5",
    "claude-fable-5",
    "claude-haiku-4-5-20251001",
], ids
print("OK: /v1/models 只报真实 cc 后端:", ", ".join(ids))
PY

echo "cc 冒烟测试通过。"
