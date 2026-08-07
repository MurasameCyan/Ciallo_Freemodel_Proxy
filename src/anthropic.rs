//! Anthropic Messages 入站协议，用于非 cc 的 transport（`http` / `workbuddy_acp`）。
//!
//! cc 上游本来就讲 Anthropic，那条路直接透传（见 [`crate::cc::passthrough_request`]）。
//! 其余 transport 只讲 OpenAI，因此在边界处翻一次进、翻一次出，中间复用既有的
//! chat 分发链路，而不是再长出一棵传输树。
use crate::sse;
use bytes::Bytes;
use serde_json::{Map, Value, json};
use uuid::Uuid;

/// Anthropic 的 content 既可以是字符串，也可以是块数组。取其中的文本。
///
/// `tool_result` 递归取它自己的 content：这条路径的下游不支持结构化工具回传，
/// 整块丢掉会让模型看不见自己刚拿到的工具输出，降级成文本至少信息还在。
fn blocks_to_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => None,
                Some("tool_result") => block.get("content").map(blocks_to_text),
                _ => block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Anthropic tools -> OpenAI tools。`input_schema` 与 `parameters` 是同一套 JSON Schema。
fn tools_to_openai(tools: &Value) -> Option<Value> {
    let tools = tools.as_array()?;
    if tools.is_empty() {
        return None;
    }
    Some(Value::Array(
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.get("name").cloned().unwrap_or(Value::Null),
                        "description": tool.get("description").cloned().unwrap_or(Value::Null),
                        "parameters": tool.get("input_schema").cloned().unwrap_or(json!({"type":"object"}))
                    }
                })
            })
            .collect(),
    ))
}

/// Anthropic Messages 请求体的最小校验，对齐 [`crate::openai::validate_chat_body`]。
pub fn validate_messages_body(body: &Value) -> Result<(), String> {
    let Some(object) = body.as_object() else {
        return Err("Request body must be a JSON object".into());
    };
    let Some(messages) = object.get("messages").and_then(Value::as_array) else {
        return Err("messages must be a non-empty array".into());
    };
    if messages.is_empty() {
        return Err("messages must be a non-empty array".into());
    }
    if !messages
        .iter()
        .all(|m| m.as_object().and_then(|o| o.get("role")).is_some())
    {
        return Err("each message must be an object with a role".into());
    }
    Ok(())
}

/// Anthropic Messages 请求 -> OpenAI Chat Completions 请求。
///
/// `system` 提到 messages 最前面变成 system 角色；工具结果块降级为文本，
/// 因为这条路径的下游（ACP / 计费 HTTP）本来就不支持结构化工具回传。
pub fn to_openai_request(body: &Value) -> Value {
    let mut messages = Vec::new();
    let system = body.get("system").map(blocks_to_text).unwrap_or_default();
    if !system.trim().is_empty() {
        messages.push(json!({"role":"system","content":system}));
    }
    for message in body
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let role = match message.get("role").and_then(Value::as_str) {
            Some("assistant") => "assistant",
            _ => "user",
        };
        let text = blocks_to_text(message.get("content").unwrap_or(&Value::Null));
        messages.push(json!({"role":role,"content":text}));
    }
    let mut out = Map::new();
    if let Some(model) = body.get("model") {
        out.insert("model".into(), model.clone());
    }
    out.insert("messages".into(), Value::Array(messages));
    for key in ["max_tokens", "temperature", "top_p", "stream"] {
        if let Some(value) = body.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(stop) = body.get("stop_sequences") {
        out.insert("stop".into(), stop.clone());
    }
    if let Some(tools) = body.get("tools").and_then(tools_to_openai) {
        out.insert("tools".into(), tools);
    }
    Value::Object(out)
}

/// OpenAI finish_reason -> Anthropic stop_reason。
pub fn to_stop_reason(finish: Option<&str>) -> Value {
    match finish {
        Some("length") => json!("max_tokens"),
        Some("tool_calls") | Some("function_call") => json!("tool_use"),
        Some(_) => json!("end_turn"),
        None => Value::Null,
    }
}

/// OpenAI usage -> Anthropic usage。
pub fn to_anthropic_usage(usage: &Value) -> Value {
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    // OpenAI 的 prompt_tokens 含缓存读取，Anthropic 的 input_tokens 不含，需要减回去。
    let input = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_sub(cached);
    json!({
        "input_tokens": input,
        "output_tokens": usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0),
        "cache_read_input_tokens": cached
    })
}

/// Anthropic 风格的消息 id，流式起始事件与补全的 tool_use 块都用它。
pub fn message_id() -> String {
    format!("msg_{}", &Uuid::new_v4().simple().to_string()[..24])
}

/// OpenAI chat.completion -> Anthropic message。
pub fn to_anthropic_message(completion: &Value, requested_model: &str) -> Value {
    let choice = completion
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let message = choice.get("message").cloned().unwrap_or(Value::Null);
    let mut content = Vec::new();
    let text = message
        .get("content")
        .map(blocks_to_text)
        .unwrap_or_default();
    if !text.is_empty() {
        content.push(json!({"type":"text","text":text}));
    }
    for call in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let function = call.get("function").cloned().unwrap_or(Value::Null);
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| json!({}));
        content.push(json!({
            "type": "tool_use",
            "id": call.get("id").cloned().unwrap_or_else(|| json!(message_id())),
            "name": function.get("name").cloned().unwrap_or(Value::Null),
            "input": arguments
        }));
    }
    json!({
        "id": completion.get("id").and_then(Value::as_str).unwrap_or("msg_freemodel"),
        "type": "message",
        "role": "assistant",
        "model": completion.get("model").and_then(Value::as_str).unwrap_or(requested_model),
        "content": content,
        "stop_reason": to_stop_reason(choice.get("finish_reason").and_then(Value::as_str)),
        "stop_sequence": Value::Null,
        "usage": to_anthropic_usage(completion.get("usage").unwrap_or(&Value::Null))
    })
}

/// 把 OpenAI SSE chunk 流增量翻成 Anthropic SSE 事件流。
///
/// Anthropic 要求成对的 `content_block_start` / `content_block_stop`，
/// 且 `message_start` 必须先于任何 delta，因此需要记住是否已经开过块。
pub struct AnthropicStreamEncoder {
    id: String,
    model: String,
    started: bool,
    block_open: bool,
    stopped: bool,
}

impl AnthropicStreamEncoder {
    /// `model` 用客户端请求的名字兜底；上游 chunk 回显了真实模型名时以后者为准。
    pub fn new(id: String, model: String) -> Self {
        Self {
            id,
            model,
            started: false,
            block_open: false,
            stopped: false,
        }
    }

    /// 首个事件对：`message_start` + `content_block_start`。
    fn start(&mut self) -> Vec<Bytes> {
        self.started = true;
        self.block_open = true;
        let (id, model) = (&self.id, &self.model);
        vec![
            sse::named(
                "message_start",
                &json!({"type":"message_start","message":{
                    "id":id,"type":"message","role":"assistant","model":model,
                    "content":[],"stop_reason":Value::Null,"stop_sequence":Value::Null,
                    "usage":{"input_tokens":0,"output_tokens":0}
                }}),
            ),
            sse::named(
                "content_block_start",
                &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            ),
        ]
    }

    /// 一段纯文本增量。ACP 上游没有 OpenAI chunk 结构，直接走这里。
    pub fn text_delta(&mut self, text: &str) -> Vec<Bytes> {
        if self.stopped {
            return Vec::new();
        }
        let mut out = Vec::new();
        if !self.started {
            out.extend(self.start());
        }
        if !text.is_empty() {
            out.push(sse::named(
                "content_block_delta",
                &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
            ));
        }
        out
    }

    /// 吃一个 OpenAI chunk，吐出对应的 Anthropic 事件。
    pub fn push(&mut self, chunk: &Value) -> Vec<Bytes> {
        if self.stopped {
            return Vec::new();
        }
        // 先认下真实模型名，`message_start` 才不会报请求名。
        if let Some(model) = chunk.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        let choice = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first());
        let text = choice
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut out = self.text_delta(text);
        if let Some(finish) = choice
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
        {
            out.extend(self.finish(to_stop_reason(Some(finish)), chunk.get("usage")));
        }
        out
    }

    /// 收尾事件：关块、`message_delta`（带 stop_reason 与 usage）、`message_stop`。
    ///
    /// 上游一个 delta 都没给就结束时（空回复）也要先补起始事件，
    /// 否则客户端收到的是一段没有 `message_start` 的孤立流。
    pub fn finish(&mut self, stop_reason: Value, usage: Option<&Value>) -> Vec<Bytes> {
        if self.stopped {
            return Vec::new();
        }
        self.stopped = true;
        let mut out = Vec::new();
        if !self.started {
            out.extend(self.start());
        }
        if self.block_open {
            out.push(sse::named(
                "content_block_stop",
                &json!({"type":"content_block_stop","index":0}),
            ));
            self.block_open = false;
        }
        let mut delta = json!({
            "type":"message_delta",
            "delta":{"stop_reason":stop_reason,"stop_sequence":Value::Null}
        });
        if let Some(usage) = usage.filter(|usage| !usage.is_null()) {
            delta["usage"] = to_anthropic_usage(usage);
        }
        out.push(sse::named("message_delta", &delta));
        out.push(sse::named(
            "message_stop",
            &json!({"type":"message_stop"}),
        ));
        out
    }

    /// 流在收到 finish_reason 之前就断了。
    pub fn is_finished(&self) -> bool {
        self.stopped
    }

    /// 错误事件，客户端据此终止而不是无限等待。
    pub fn error(&mut self, message: &str) -> Bytes {
        self.stopped = true;
        error_event(message)
    }
}

/// Anthropic 流的错误事件。透传路径没有编码器状态可用，所以做成自由函数。
pub fn error_event(message: &str) -> Bytes {
    sse::named(
        "error",
        &json!({"type":"error","error":{"type":"api_error","message":message}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payloads(events: &[Bytes]) -> Vec<Value> {
        events
            .iter()
            .map(|event| {
                let text = String::from_utf8_lossy(event);
                let data = text.split("data: ").nth(1).unwrap().trim().to_string();
                serde_json::from_str(&data).unwrap()
            })
            .collect()
    }

    fn types(events: &[Bytes]) -> Vec<String> {
        payloads(events)
            .iter()
            .map(|value| value["type"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn hoists_system_and_flattens_blocks() {
        let request = to_openai_request(&json!({
            "model": "claude-opus-5",
            "system": [{"type":"text","text":"be terse"}],
            "max_tokens": 64,
            "stop_sequences": ["STOP"],
            "messages": [{"role":"user","content":[{"type":"text","text":"hi"}]}]
        }));
        assert_eq!(
            request["messages"],
            json!([
                {"role":"system","content":"be terse"},
                {"role":"user","content":"hi"}
            ])
        );
        assert_eq!(request["stop"], json!(["STOP"]));
        assert_eq!(request["max_tokens"], json!(64));
    }

    #[test]
    fn tool_result_blocks_survive_as_text() {
        let request = to_openai_request(&json!({
            "messages": [{"role":"user","content":[
                {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"42"}]}
            ]}]
        }));
        assert_eq!(request["messages"][0]["content"], json!("42"));
    }

    #[test]
    fn tools_carry_the_schema_across() {
        let request = to_openai_request(&json!({
            "messages": [{"role":"user","content":"hi"}],
            "tools": [{"name":"now","description":"clock","input_schema":{"type":"object"}}]
        }));
        assert_eq!(
            request["tools"][0],
            json!({"type":"function","function":{
                "name":"now","description":"clock","parameters":{"type":"object"}
            }})
        );
    }

    #[test]
    fn cached_tokens_leave_input_tokens() {
        // OpenAI 的 prompt_tokens 含缓存读取，Anthropic 的 input_tokens 不含。
        let usage = to_anthropic_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 7,
            "prompt_tokens_details": {"cached_tokens": 90}
        }));
        assert_eq!(usage["input_tokens"], json!(10));
        assert_eq!(usage["cache_read_input_tokens"], json!(90));
        assert_eq!(usage["output_tokens"], json!(7));
    }

    #[test]
    fn tool_calls_become_tool_use_blocks() {
        let message = to_anthropic_message(
            &json!({
                "id": "chatcmpl-1",
                "model": "claude-opus-5",
                "choices": [{"finish_reason":"tool_calls","message":{
                    "content": "checking",
                    "tool_calls": [{"id":"call_1","function":{"name":"now","arguments":"{\"tz\":\"utc\"}"}}]
                }}]
            }),
            "requested",
        );
        assert_eq!(message["stop_reason"], json!("tool_use"));
        assert_eq!(message["content"][0], json!({"type":"text","text":"checking"}));
        assert_eq!(
            message["content"][1],
            json!({"type":"tool_use","id":"call_1","name":"now","input":{"tz":"utc"}})
        );
    }

    #[test]
    fn stream_opens_and_closes_every_block() {
        let mut encoder = AnthropicStreamEncoder::new("msg_1".into(), "requested".into());
        let mut events = encoder.push(&json!({
            "model": "claude-opus-5",
            "choices": [{"delta":{"content":"hi"},"finish_reason":Value::Null}]
        }));
        assert_eq!(
            types(&events),
            ["message_start", "content_block_start", "content_block_delta"]
        );
        // 上游回显的真实后端名要盖掉请求名。
        assert_eq!(payloads(&events)[0]["message"]["model"], json!("claude-opus-5"));
        events = encoder.push(&json!({
            "choices": [{"delta":{},"finish_reason":"stop"}],
            "usage": {"prompt_tokens":3,"completion_tokens":1}
        }));
        assert_eq!(
            types(&events),
            ["content_block_stop", "message_delta", "message_stop"]
        );
        assert_eq!(payloads(&events)[1]["delta"]["stop_reason"], json!("end_turn"));
        assert_eq!(payloads(&events)[1]["usage"]["input_tokens"], json!(3));
        assert!(encoder.is_finished());
        assert!(encoder.push(&json!({"choices":[]})).is_empty());
    }

    #[test]
    fn empty_stream_still_starts_before_it_stops() {
        let mut encoder = AnthropicStreamEncoder::new("msg_1".into(), "claude-opus-5".into());
        let events = encoder.finish(json!("end_turn"), None);
        assert_eq!(
            types(&events),
            [
                "message_start",
                "content_block_start",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
    }
}
