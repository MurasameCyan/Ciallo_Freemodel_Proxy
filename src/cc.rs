//! OpenAI Chat Completions <-> Anthropic Messages 转换，用于 cc.freemodel.dev。
//!
//! 上游只有三个真实后端（claude-opus-5 / claude-fable-5 / claude-haiku-4-5-20251001），
//! 其余别名由上游静默改换并在 `message_start.model` 如实回显。上游共享容器池占满时
//! 返回 500 `Maximum number of running container instances exceeded`，与账号配额无关，
//! 因此需要降级链而非直接把 500 透传给客户端。
use serde_json::{Map, Value, json};

/// 上游确认可直达的后端，按能力从高到低。
pub const CC_BACKENDS: [&str; 3] = ["claude-opus-5", "claude-fable-5", "claude-haiku-4-5-20251001"];

/// 请求模型撞上游 500 时的降级顺序：先试请求的模型，再依次退到其它可用后端。
///
/// 非 `claude-*` 名字（例如客户端不传 model 时被归一出的 `gpt-5.6-sol`）上游不认，
/// 送过去只会白跑一轮，因此直接从真实后端起步。
pub fn fallback_chain(model: &str) -> Vec<String> {
    let mut chain = Vec::new();
    if model.starts_with("claude-") {
        chain.push(model.to_string());
    }
    for backend in CC_BACKENDS {
        if !chain.iter().any(|m| m == backend) {
            chain.push(backend.to_string());
        }
    }
    chain
}

/// 上游 500 是否为容器池占满（可降级重试），而非真正的上游故障。
pub fn is_pool_exhausted(body: &str) -> bool {
    body.contains("Maximum number of running container instances exceeded")
}

/// 第 `index` 次重试前的等待时长（`index` 从 1 起，0 是首次请求不等待）。
///
/// 容器池是网关级上限，换 model 并不会腾出实例，真正让请求成功的是等一会儿：
/// 实测零间隔连打必然三连撞满池，退避几秒后同一个请求就能拿到 200。
/// 单次上限 6 秒，最长的一条链（请求名不在后端表里时共 4 个候选）最坏多等 12 秒。
pub fn pool_backoff(index: usize) -> std::time::Duration {
    std::time::Duration::from_secs((2 * index as u64).min(6))
}

/// 上游容器在客户端 system 之前注入了自己的 agent harness prompt（实测身份在 Kiro 与
/// Claude Code 之间随容器变化），它不计入 usage，也无法从客户端删除：system 覆盖、
/// user 轮尾部指令、预填 assistant 轮、显式「忽略先前指令」的元指令全部无效。
///
/// 真正会弄坏客户端的不是身份，而是它让模型以为自己有文件工具，于是把
/// `<function_calls>` / `<tool_call>` 连同编造的工具结果当**正文**吐出来，
/// 有时还带出上游容器的本地路径。抽象的元指令压不住（实测 3/3 仍泄漏），
/// 直接陈述事实可以（实测 6/6 干净），因此这里只做一句事实声明。
const GUARD_NO_TOOLS: &str = "You have no tools, no filesystem access and no shell in this session. Never emit <function_calls>, <invoke>, <tool_call> or <function_response> markup as text, and never invent tool results. If a request would need tools, say so in plain text.";

/// 客户端自带 tools 时不能说「你没有工具」，否则会压掉合法的 tool_use。
const GUARD_WITH_TOOLS: &str = "The only tools available are the ones declared in this request; you have no filesystem access and no shell beyond them. Invoke them through the structured tool-call mechanism only. Never emit <function_calls>, <invoke>, <tool_call> or <function_response> markup as text, and never invent tool results.";

/// 按客户端是否自带工具选择对应的 guard 文本。
pub fn prompt_guard(has_tools: bool) -> &'static str {
    if has_tools {
        GUARD_WITH_TOOLS
    } else {
        GUARD_NO_TOOLS
    }
}

/// 把 guard 追加到客户端 system 之后（Anthropic 的 system 既可以是字符串也可以是块数组）。
///
/// 追加而非前置：guard 要压的是更靠前的注入 prompt，位置越靠后越有效；
/// 客户端自己的指令仍在 guard 之前，实测不受影响。
pub fn guard_system(system: Option<&Value>, has_tools: bool) -> Value {
    let guard = prompt_guard(has_tools);
    match system {
        Some(Value::Array(blocks)) => {
            let mut blocks = blocks.clone();
            blocks.push(json!({"type":"text","text":guard}));
            Value::Array(blocks)
        }
        Some(Value::String(text)) if !text.trim().is_empty() => {
            Value::String(format!("{text}\n\n{guard}"))
        }
        _ => Value::String(guard.to_string()),
    }
}

fn content_to_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| part.as_str().map(str::to_string))
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// OpenAI Chat 请求体 -> Anthropic Messages 请求体。
///
/// Anthropic 要求 system 提到顶层，且 messages 必须 user/assistant 交替开头为 user。
/// 上游实测允许省略 `max_tokens`，但显式给出更可控。
pub fn to_anthropic_request(body: &Value, model: &str, guard: bool) -> Value {
    let mut system = Vec::new();
    let mut messages = Vec::new();
    for message in body
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
        let text = content_to_text(message.get("content").unwrap_or(&Value::Null));
        match role {
            "system" | "developer" => system.push(text),
            "assistant" => messages.push(json!({"role":"assistant","content":text})),
            _ => messages.push(json!({"role":"user","content":text})),
        }
    }
    if messages.is_empty() {
        messages.push(json!({"role":"user","content":""}));
    }
    let mut out = Map::new();
    out.insert("model".into(), Value::String(model.into()));
    out.insert("messages".into(), Value::Array(messages));
    let client_system = if system.is_empty() {
        None
    } else {
        Some(Value::String(system.join("\n\n")))
    };
    let system = if guard {
        // OpenAI 客户端不会自带 Anthropic tools，按无工具变体处理。
        Some(guard_system(client_system.as_ref(), false))
    } else {
        client_system
    };
    if let Some(system) = system {
        out.insert("system".into(), system);
    }
    let max_tokens = body
        .get("max_tokens")
        .or_else(|| body.get("max_completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(8192);
    out.insert("max_tokens".into(), json!(max_tokens));
    for key in ["temperature", "top_p", "stop_sequences"] {
        if let Some(value) = body.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(stop) = body.get("stop") {
        let sequences = match stop {
            Value::String(one) => vec![Value::String(one.clone())],
            Value::Array(many) => many.clone(),
            _ => Vec::new(),
        };
        if !sequences.is_empty() {
            out.insert("stop_sequences".into(), Value::Array(sequences));
        }
    }
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        out.insert("stream".into(), Value::Bool(true));
    }
    Value::Object(out)
}

/// Anthropic 客户端直连 `/v1/messages` 时的请求处理：换后端名，按需加 guard。
///
/// 其余字段（tools、tool_result、图片、cache_control、thinking 等）原样透传——
/// 上游本来就讲 Anthropic，任何翻译都只会丢信息。
pub fn passthrough_request(body: &Value, model: &str, guard: bool) -> Value {
    let mut out = body.clone();
    let object = match out.as_object_mut() {
        Some(object) => object,
        None => return out,
    };
    object.insert("model".into(), Value::String(model.into()));
    if guard {
        let has_tools = body
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty());
        let system = guard_system(body.get("system"), has_tools);
        object.insert("system".into(), system);
    }
    out
}

fn map_stop_reason(reason: Option<&str>) -> Value {
    match reason {
        Some("end_turn") | Some("stop_sequence") => json!("stop"),
        Some("max_tokens") => json!("length"),
        Some("tool_use") => json!("tool_calls"),
        Some(_) => json!("stop"),
        None => Value::Null,
    }
}

/// Anthropic usage -> OpenAI usage，保留缓存读写以便累加真实消耗。
pub fn to_openai_usage(usage: &Value) -> Value {
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cached = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "prompt_tokens": input + cached,
        "completion_tokens": output,
        "total_tokens": input + cached + output,
        "prompt_tokens_details": {"cached_tokens": cached}
    })
}

/// Anthropic 非流式响应 -> OpenAI chat.completion。
///
/// `model` 取上游回显值而非客户端请求值，因为上游会静默改换模型，
/// 透传真实后端名让客户端知道自己实际用的是什么。
pub fn to_openai_completion(response: &Value, requested_model: &str) -> Value {
    let text = content_to_text(response.get("content").unwrap_or(&Value::Null));
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(requested_model);
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-freemodel");
    let finish = map_stop_reason(response.get("stop_reason").and_then(Value::as_str));
    json!({
        "id": id,
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role":"assistant","content":text},
            "finish_reason": finish
        }],
        "usage": to_openai_usage(response.get("usage").unwrap_or(&Value::Null))
    })
}

/// Anthropic SSE 事件流的增量状态机产物。
#[derive(Clone, Debug, PartialEq)]
pub enum CcEvent {
    /// 首个 `message_start`，携带上游回显的真实模型名。
    Model(String),
    Text(String),
    /// `message_delta` 的 stop_reason 与累计 usage。
    Finish(Value, Value),
    Done,
}

/// 解析一条 Anthropic SSE `data:` 负载。返回 `None` 表示该事件无需转发。
pub fn parse_cc_event(payload: &str) -> Option<CcEvent> {
    let payload = payload.trim();
    if payload.is_empty() {
        return None;
    }
    if payload == "[DONE]" {
        return Some(CcEvent::Done);
    }
    let event: Value = serde_json::from_str(payload).ok()?;
    match event.get("type").and_then(Value::as_str)? {
        "message_start" => event
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(Value::as_str)
            .map(|model| CcEvent::Model(model.to_string())),
        "content_block_delta" => event
            .get("delta")
            .and_then(|d| d.get("text"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| CcEvent::Text(text.to_string())),
        "message_delta" => {
            let finish = map_stop_reason(
                event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str),
            );
            let usage = event.get("usage").cloned().unwrap_or(Value::Null);
            Some(CcEvent::Finish(finish, usage))
        }
        "message_stop" => Some(CcEvent::Done),
        "error" => Some(CcEvent::Done),
        _ => None,
    }
}
