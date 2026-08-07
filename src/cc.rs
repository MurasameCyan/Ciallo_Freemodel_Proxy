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
pub fn to_anthropic_request(body: &Value, model: &str) -> Value {
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
    if !system.is_empty() {
        out.insert("system".into(), Value::String(system.join("\n\n")));
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
