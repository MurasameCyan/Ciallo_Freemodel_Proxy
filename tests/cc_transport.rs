use freemodel_workbuddy_proxy::cc::{
    CC_BACKENDS, CcEvent, fallback_chain, is_pool_exhausted, parse_cc_event, passthrough_request,
    pool_backoff, prompt_guard, to_anthropic_request, to_openai_completion, to_openai_usage,
};
use serde_json::{Value, json};
use std::time::Duration;

#[test]
fn system_messages_move_to_top_level_and_join() {
    let body = json!({
        "model": "claude-opus-5",
        "messages": [
            {"role":"system","content":"be terse"},
            {"role":"user","content":"hi"},
            {"role":"developer","content":"no emoji"},
        ]
    });
    let out = to_anthropic_request(&body, "claude-opus-5", false);
    assert_eq!(out["system"], json!("be terse\n\nno emoji"));
    assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    assert_eq!(out["messages"][0]["role"], json!("user"));
}

#[test]
fn multipart_content_flattens_to_text() {
    let body = json!({
        "messages": [{"role":"user","content":[
            {"type":"text","text":"a"},
            {"type":"text","text":"b"}
        ]}]
    });
    let out = to_anthropic_request(&body, "claude-opus-5", false);
    assert_eq!(out["messages"][0]["content"], json!("ab"));
}

#[test]
fn max_tokens_is_always_present_for_anthropic() {
    // Anthropic 要求 max_tokens；OpenAI 客户端常常不发，缺省不能落空。
    let out = to_anthropic_request(&json!({"messages":[]}), "claude-opus-5", false);
    assert_eq!(out["max_tokens"], json!(8192));

    let explicit = to_anthropic_request(
        &json!({"max_completion_tokens": 64, "messages": []}),
        "claude-opus-5",
        false,
    );
    assert_eq!(explicit["max_tokens"], json!(64));
}

#[test]
fn empty_messages_still_produce_a_user_turn() {
    // Anthropic 拒绝空 messages，必须兜底，否则代理会把 400 转嫁给客户端。
    let out = to_anthropic_request(&json!({"messages": []}), "claude-opus-5", false);
    assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    assert_eq!(out["messages"][0]["role"], json!("user"));
}

#[test]
fn openai_stop_string_becomes_anthropic_stop_sequences() {
    let out = to_anthropic_request(&json!({"stop":"END","messages":[]}), "claude-opus-5", false);
    assert_eq!(out["stop_sequences"], json!(["END"]));
}

#[test]
fn fallback_chain_starts_with_requested_model_without_duplicates() {
    let chain = fallback_chain("claude-opus-4-8");
    assert_eq!(chain[0], "claude-opus-4-8");
    assert_eq!(chain.len(), 1 + CC_BACKENDS.len());

    // 请求的模型本身就是后端时不应重复出现。
    let direct = fallback_chain("claude-opus-5");
    assert_eq!(direct[0], "claude-opus-5");
    assert_eq!(direct.len(), CC_BACKENDS.len());
    assert_eq!(
        direct.iter().filter(|m| *m == "claude-opus-5").count(),
        1,
        "降级链不能重复同一个后端"
    );
}

#[test]
fn non_claude_model_names_resolve_to_the_default_backend() {
    // 客户端不传 model 时会被归一成 gpt-5.6-sol，cc 上游不认这个名字。
    assert_eq!(fallback_chain("gpt-5.6-sol"), CC_BACKENDS.map(String::from));
    assert_eq!(fallback_chain(""), CC_BACKENDS.map(String::from));
    // Claude 别名要原样送上游，由上游自己决定换成哪个后端。
    assert_eq!(fallback_chain("claude-sonnet-5")[0], "claude-sonnet-5");
}

#[test]
fn only_container_pool_exhaustion_is_retryable() {
    assert!(is_pool_exhausted(
        "Failed to start container: Maximum number of running container instances exceeded. Try again later"
    ));
    assert!(!is_pool_exhausted("{\"error\":\"Unauthorized - Invalid token\"}"));
}

#[test]
fn retries_wait_because_the_pool_is_gateway_wide() {
    // 首次请求不等；换后端并不腾出实例，零间隔重试只会再撞一次满池。
    assert_eq!(pool_backoff(0), Duration::ZERO);
    assert!(pool_backoff(1) >= Duration::from_secs(1));
    assert!(pool_backoff(2) > pool_backoff(1));
    // 有上限，否则最长的链会把客户端拖过读超时。
    assert!(pool_backoff(99) <= Duration::from_secs(6));
}

#[test]
fn completion_reports_the_backend_the_upstream_actually_used() {
    // 上游会把 claude-sonnet-5 静默换成 claude-opus-5，客户端应看到真实后端。
    let upstream = json!({
        "id": "msg_1",
        "model": "claude-opus-5",
        "content": [{"type":"text","text":"OK"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 24, "output_tokens": 37}
    });
    let out = to_openai_completion(&upstream, "claude-sonnet-5");
    assert_eq!(out["model"], json!("claude-opus-5"));
    assert_eq!(out["choices"][0]["message"]["content"], json!("OK"));
    assert_eq!(out["choices"][0]["finish_reason"], json!("stop"));
    assert_eq!(out["usage"]["total_tokens"], json!(61));
}

#[test]
fn max_tokens_stop_reason_maps_to_length() {
    let upstream = json!({"content": [], "stop_reason": "max_tokens", "usage": {}});
    let out = to_openai_completion(&upstream, "claude-opus-5");
    assert_eq!(out["choices"][0]["finish_reason"], json!("length"));
}

#[test]
fn cached_tokens_count_toward_prompt_total() {
    let usage = to_openai_usage(&json!({
        "input_tokens": 10,
        "output_tokens": 5,
        "cache_read_input_tokens": 90
    }));
    assert_eq!(usage["prompt_tokens"], json!(100));
    assert_eq!(usage["total_tokens"], json!(105));
    assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], json!(90));
}

#[test]
fn stream_events_expose_model_text_and_finish() {
    assert_eq!(
        parse_cc_event(r#"{"type":"message_start","message":{"model":"claude-fable-5"}}"#),
        Some(CcEvent::Model("claude-fable-5".into()))
    );
    assert_eq!(
        parse_cc_event(r#"{"type":"content_block_delta","delta":{"text":"hi"}}"#),
        Some(CcEvent::Text("hi".into()))
    );
    assert_eq!(parse_cc_event(r#"{"type":"message_stop"}"#), Some(CcEvent::Done));
    assert_eq!(parse_cc_event("[DONE]"), Some(CcEvent::Done));

    // ping/无关事件不得产生输出块。
    assert_eq!(parse_cc_event(r#"{"type":"ping"}"#), None);
    assert_eq!(parse_cc_event(""), None);
    assert_eq!(parse_cc_event("not json"), None);

    match parse_cc_event(
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
    ) {
        Some(CcEvent::Finish(reason, usage)) => {
            assert_eq!(reason, json!("stop"));
            assert_eq!(usage["output_tokens"], json!(7));
        }
        other => panic!("expected Finish, got {other:?}"),
    }
}

#[test]
fn empty_text_delta_is_not_forwarded() {
    // 空 delta 会让客户端收到无意义的空块。
    assert_eq!(
        parse_cc_event(r#"{"type":"content_block_delta","delta":{"text":""}}"#),
        None
    );
}

#[test]
fn guard_goes_after_the_client_system_not_before() {
    // guard 要压的是更靠前的注入 prompt，位置越靠后越有效；客户端指令必须仍在它之前。
    let out = to_anthropic_request(
        &json!({"messages":[{"role":"system","content":"be terse"},{"role":"user","content":"hi"}]}),
        "claude-opus-5",
        true,
    );
    let system = out["system"].as_str().unwrap();
    assert!(system.starts_with("be terse"));
    assert!(system.ends_with(prompt_guard(false)));
}

#[test]
fn guard_is_the_whole_system_when_the_client_sends_none() {
    let out = to_anthropic_request(&json!({"messages":[]}), "claude-opus-5", true);
    assert_eq!(out["system"], json!(prompt_guard(false)));

    // 关掉开关就完全不碰 system。
    let bare = to_anthropic_request(&json!({"messages":[]}), "claude-opus-5", false);
    assert_eq!(bare.get("system"), None);
}

#[test]
fn passthrough_only_swaps_the_model() {
    // 上游本来就讲 Anthropic，除 model 外任何字段都不该被改写或丢弃。
    let body = json!({
        "model": "claude-sonnet-5",
        "messages": [{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"42"}]}],
        "tools": [{"name":"now","input_schema":{"type":"object"}}],
        "metadata": {"user_id":"u1"},
        "thinking": {"type":"enabled","budget_tokens":1024}
    });
    let out = passthrough_request(&body, "claude-opus-5", false);
    assert_eq!(out["model"], json!("claude-opus-5"));
    for key in ["messages", "tools", "metadata", "thinking"] {
        assert_eq!(out[key], body[key], "{key} 不应被改写");
    }
    assert_eq!(out.get("system"), None);
}

#[test]
fn guard_picks_the_variant_matching_the_client_tools() {
    // 客户端自带 tools 时说「你没有工具」会压掉合法的 tool_use。
    let with_tools = passthrough_request(
        &json!({"messages":[],"tools":[{"name":"now","input_schema":{}}]}),
        "claude-opus-5",
        true,
    );
    assert_eq!(with_tools["system"], json!(prompt_guard(true)));

    let without = passthrough_request(&json!({"messages":[],"tools":[]}), "claude-opus-5", true);
    assert_eq!(without["system"], json!(prompt_guard(false)));
}

#[test]
fn guard_appends_a_block_when_system_is_a_block_array() {
    // Anthropic 的 system 既可以是字符串也可以是块数组，后者常带 cache_control。
    let client = json!([{"type":"text","text":"be terse","cache_control":{"type":"ephemeral"}}]);
    let out = passthrough_request(
        &json!({"messages":[],"system":client.clone()}),
        "claude-opus-5",
        true,
    );
    let blocks = out["system"].as_array().unwrap();
    assert_eq!(blocks[0], client[0], "客户端的块要原样保留");
    assert_eq!(blocks[1], json!({"type":"text","text":prompt_guard(false)}));
}

#[test]
fn guard_survives_a_non_object_body() {
    // 上游会替我们拒绝非法请求体，代理不该在这里 panic。
    assert_eq!(
        passthrough_request(&json!("nonsense"), "claude-opus-5", true),
        Value::String("nonsense".into())
    );
}
