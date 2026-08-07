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

# Anthropic 入站端点也必须在镜像里存在且同样受鉴权保护。
# 空 messages 在校验阶段就被拒，不会打上游，因此不花额度。
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/v1/messages" \
  -H 'Content-Type: application/json' -d '{"messages":[]}')
if [[ "$code" != "401" ]]; then
    echo "FAIL: 未鉴权的 /v1/messages 返回 $code，应为 401" >&2
    exit 1
fi
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/v1/messages" \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' -d '{"messages":[]}')
if [[ "$code" != "400" ]]; then
    echo "FAIL: 空 messages 的 /v1/messages 返回 $code，应为 400（路由缺失会是 404/405）" >&2
    exit 1
fi
echo "OK: /v1/messages 已挂载且受鉴权保护"

# Web 设定页必须真的打包进镜像：它是 include_str! 进二进制的，漏了会静默变成 404。
setup_type=$(curl -fsS -o /dev/null -w '%{content_type}' "http://127.0.0.1:$PORT/setup")
case "$setup_type" in
    text/html*) ;;
    *) echo "FAIL: /setup 的 content-type 是 '$setup_type'，应为 text/html" >&2; exit 1 ;;
esac
echo "OK: /setup 返回 HTML"

# 写上游 key 的端点绝不能免鉴权：镜像绑 0.0.0.0，开放它等于让同网段任何人换你的 key。
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/setup/key" \
  -H 'Content-Type: application/json' -d '{"key":"fe_smoke_placeholder_key"}')
if [[ "$code" != "401" ]]; then
    echo "FAIL: 未鉴权的 /setup/key 返回 $code，应为 401" >&2
    exit 1
fi

# 换行必须在入口被挡掉，否则会被拼进 Authorization header。
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/setup/key" \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"key":"fe_smoke\nX-Injected: 1"}')
if [[ "$code" != "400" ]]; then
    echo "FAIL: 含换行的 key 返回 $code，应为 400" >&2
    exit 1
fi

# 存一把假 key，验证整条写盘路径在真镜像里跑得通（容器随后即删，不涉及真实凭据）。
saved=$(curl -fsS -X POST "http://127.0.0.1:$PORT/setup/key" \
  -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
  -d '{"key":"fe_smoke_placeholder_key"}')
python3 - "$saved" <<'PY'
import json, sys
saved = json.loads(sys.argv[1])
assert saved["saved"] is True, saved
# 响应会进浏览器历史和日志，只能回掩码。
assert saved["freemodel_key"] == "fe_s••••_key", saved
assert "fe_smoke_placeholder_key" not in sys.argv[1], saved
print("OK: /setup/key 已写入并只回显掩码:", saved["freemodel_key"])
PY

# 没有 PROXY_API_KEY 时必须拒绝启动。镜像绑 0.0.0.0，空 key 等于关闭鉴权，
# 而这个容器持有上游额度和 /setup/key 的写入权限。曾经真的这样跑在公网上过。
if docker run --rm "$IMAGE" >/tmp/no-key.log 2>&1; then
    echo "FAIL: 未设置 PROXY_API_KEY 时容器竟然正常启动了" >&2
    exit 1
fi
if ! grep -qF 'PROXY_API_KEY' /tmp/no-key.log; then
    echo "FAIL: 拒绝启动的原因没有提到 PROXY_API_KEY：" >&2
    cat /tmp/no-key.log >&2
    exit 1
fi
echo "OK: 缺少 PROXY_API_KEY 时拒绝启动，且说明了原因"

echo "cc 冒烟测试通过。"
