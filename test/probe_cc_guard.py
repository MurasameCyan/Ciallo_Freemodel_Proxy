#!/usr/bin/env python3
"""对真实 cc.freemodel.dev 验证 guard 是否压住了上游注入 prompt 的工具幻觉。

会真实消耗额度，因此不进 CI，只在手工验证时跑：

    python3 test/probe_cc_guard.py <key-file>

key 文件取第一行。上游是共享容器池，500 是占满而非失败，脚本会自动重试。
"""

import json
import subprocess
import sys
import time

URL = "https://cc.freemodel.dev/v1/messages"
ASK = "列出当前目录下的文件"
CLIENT_SYS = [{"type": "text", "text": "You are a terse assistant. Reply in 简体中文."}]

# 与 src/cc.rs 的 GUARD_NO_TOOLS / GUARD_WITH_TOOLS 逐字一致，改动一处要同步另一处。
GUARD = (
    "You have no tools, no filesystem access and no shell in this session. "
    "Never emit <function_calls>, <invoke>, <tool_call> or <function_response> "
    "markup as text, and never invent tool results. If a request would need tools, "
    "say so in plain text."
)
GUARD_WITH_TOOLS = (
    "The only tools available are the ones declared in this request; you have no "
    "filesystem access and no shell beyond them. Invoke them through the structured "
    "tool-call mechanism only. Never emit <function_calls>, <invoke>, <tool_call> or "
    "<function_response> markup as text, and never invent tool results."
)
TOOLS = [
    {
        "name": "list_dir",
        "description": "列出目录内容",
        "input_schema": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        },
    }
]
MARKS = ["<function_calls>", "<invoke", "<tool_call", "<function_response", "antml:"]


def post(key, body, attempts=6):
    """打一次上游；500 容器池占满就重试，其余状态原样返回。"""
    payload = json.dumps(body, ensure_ascii=False).encode("utf-8")
    for attempt in range(attempts):
        result = subprocess.run(
            [
                "curl", "-sS", "-m", "240", "-w", "\n%{http_code}", "-X", "POST", URL,
                "-H", f"Authorization: Bearer {key}",
                "-H", "anthropic-version: 2023-06-01",
                "-H", "Content-Type: application/json",
                "--data-binary", "@-",
            ],
            input=payload,
            capture_output=True,
        )
        text = result.stdout.decode("utf-8", "replace")
        raw, _, status = text.rpartition("\n")
        if status.strip() != "500":
            return status.strip(), raw
        time.sleep(4 * (attempt + 1))
    return "500", raw


def report(label, key, system, tools=None, stream=False):
    body = {
        "model": "claude-opus-5",
        "max_tokens": 400,
        "system": system,
        "messages": [{"role": "user", "content": ASK}],
    }
    if tools:
        body["tools"] = tools
    if stream:
        body["stream"] = True
    status, raw = post(key, body)
    print(f"===== {label} (http={status}) =====")
    if status != "200":
        print(raw[:200])
        return None
    if stream:
        events = [
            line.removeprefix("event: ")
            for line in raw.splitlines()
            if line.startswith("event: ")
        ]
        print("events:", " ".join(events))
        text = "".join(
            json.loads(line.removeprefix("data: "))
            .get("delta", {})
            .get("text", "")
            for line in raw.splitlines()
            if line.startswith("data: ") and '"text_delta"' in line
        )
    else:
        message = json.loads(raw)
        blocks = message.get("content", [])
        print("backend:", message.get("model"), "| stop:", message.get("stop_reason"))
        print("block types:", [b.get("type") for b in blocks])
        for block in blocks:
            if block.get("type") == "tool_use":
                print("tool_use:", block.get("name"), block.get("input"))
        text = "".join(b.get("text", "") for b in blocks if b.get("type") == "text")
    leaks = [m for m in MARKS if m in text]
    print("LEAK:", leaks or "none")
    print("reply:", text[:400].replace("\n", " "))
    return leaks


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    with open(sys.argv[1], encoding="utf-8") as handle:
        key = handle.readline().strip()

    report("A 无 guard（对照）", key, CLIENT_SYS)
    report("B 有 guard", key, CLIENT_SYS + [{"type": "text", "text": GUARD}])
    report(
        "C 有 guard + 客户端 tools",
        key,
        CLIENT_SYS + [{"type": "text", "text": GUARD_WITH_TOOLS}],
        tools=TOOLS,
    )
    report(
        "D 有 guard + 流式",
        key,
        CLIENT_SYS + [{"type": "text", "text": GUARD}],
        stream=True,
    )


if __name__ == "__main__":
    main()
