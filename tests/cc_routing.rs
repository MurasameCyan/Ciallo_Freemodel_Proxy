use axum::{
    Json, Router,
    body::Body,
    extract::{State, connect_info::MockConnectInfo},
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::post,
};
use freemodel_workbuddy_proxy::{
    cc::CC_BACKENDS,
    config::Config,
    server::{AppState, router},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use tempfile::tempdir;
use tower::ServiceExt;

/// cc.freemodel.dev 的替身：记录收到的 Anthropic 请求，按脚本顺序回放响应。
#[derive(Clone, Default)]
struct Upstream {
    requests: Arc<Mutex<Vec<(HeaderMap, Value)>>>,
    scripted: Arc<Mutex<VecDeque<(StatusCode, String, String)>>>,
}

impl Upstream {
    fn bodies(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(_, body)| body.clone())
            .collect()
    }
    fn header(&self, index: usize, name: &str) -> String {
        self.requests.lock().unwrap()[index]
            .0
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }
}

async fn mock_cc(script: Vec<(StatusCode, &str, &str)>) -> (String, Upstream) {
    async fn messages(
        State(state): State<Upstream>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        state.requests.lock().unwrap().push((headers, body));
        match state.scripted.lock().unwrap().pop_front() {
            Some((status, content_type, body)) => {
                (status, [("content-type", content_type)], body).into_response()
            }
            // 脚本用尽说明代理比预期多打了一次上游，让测试看见这一点。
            None => (
                StatusCode::IM_A_TEAPOT,
                [("content-type", "application/json".to_string())],
                json!({"error":{"message":"unscripted upstream call"}}).to_string(),
            )
                .into_response(),
        }
    }
    let state = Upstream {
        requests: Arc::default(),
        scripted: Arc::new(Mutex::new(
            script
                .into_iter()
                .map(|(status, content_type, body)| {
                    (status, content_type.to_string(), body.to_string())
                })
                .collect(),
        )),
    };
    let app = Router::new()
        .route("/v1/messages", post(messages))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/v1"), state)
}

fn cc_state(base_url: &str, api_key: &str) -> (tempfile::TempDir, AppState) {
    let root = tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let env = HashMap::from([
        ("HOME".into(), root.path().to_string_lossy().to_string()),
        ("FREEMODEL_BASE_URL".into(), base_url.to_string()),
        ("FREEMODEL_TRANSPORT".into(), "cc_anthropic".into()),
        ("FREEMODEL_API_KEY".into(), api_key.to_string()),
        (
            "PROXY_DEFAULT_PROJECT".into(),
            project.to_string_lossy().to_string(),
        ),
        (
            "PROXY_SESSION_STORE".into(),
            root.path()
                .join("sessions.json")
                .to_string_lossy()
                .to_string(),
        ),
        (
            "PROXY_RUNTIME_DIR".into(),
            root.path().join("runtime").to_string_lossy().to_string(),
        ),
    ]);
    let config = Config::load_with_env(root.path(), &env).unwrap();
    let state = AppState::new(config).unwrap();
    (root, state)
}

async fn send(state: AppState, request: Request<Body>) -> (StatusCode, String) {
    let response = router(state)
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            40000,
        ))))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn post_json(state: AppState, route: &str, body: Value) -> (StatusCode, String) {
    let request = Request::post(route)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    send(state, request).await
}

fn user_chat(model: &str) -> Value {
    json!({"model":model,"messages":[{"role":"user","content":"hi"}]})
}

fn sse_events(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).unwrap())
        .collect()
}

const ANTHROPIC_REPLY: &str = r#"{"id":"msg_01","type":"message","role":"assistant","model":"claude-fable-5","content":[{"type":"text","text":"pong"}],"stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":2,"cache_read_input_tokens":3}}"#;

const POOL_EXHAUSTED: &str =
    r#"{"error":{"message":"Maximum number of running container instances exceeded"}}"#;

#[tokio::test]
async fn cc_chat_speaks_anthropic_upstream_and_openai_downstream() {
    let (base, upstream) = mock_cc(vec![(StatusCode::OK, "application/json", ANTHROPIC_REPLY)]).await;
    let (_root, state) = cc_state(&base, "test-key");
    let (status, body) = post_json(
        state,
        "/v1/chat/completions",
        json!({
            "model":"claude-opus-5",
            "messages":[
                {"role":"system","content":"be terse"},
                {"role":"user","content":"ping"}
            ]
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    // cc 只认 Bearer key，不需要任何登录态；漏掉版本头上游会 400。
    assert_eq!(upstream.header(0, "authorization"), "Bearer test-key");
    assert_eq!(upstream.header(0, "anthropic-version"), "2023-06-01");
    let sent = &upstream.bodies()[0];
    assert_eq!(sent["model"], "claude-opus-5");
    assert_eq!(sent["system"], "be terse");
    assert_eq!(sent["messages"][0]["content"], "ping");
    assert!(sent["max_tokens"].is_u64());
    assert!(sent.get("stream").is_none(), "{sent}");

    let out: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(out["object"], "chat.completion");
    // 上游把 opus 换成了 fable，客户端应看到真实后端而不是自己请求的名字。
    assert_eq!(out["model"], "claude-fable-5");
    assert_eq!(out["choices"][0]["message"]["content"], "pong");
    assert_eq!(out["choices"][0]["finish_reason"], "stop");
    assert_eq!(out["usage"]["prompt_tokens"], 10);
    assert_eq!(out["usage"]["completion_tokens"], 2);
    assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
}

#[tokio::test]
async fn container_pool_exhaustion_falls_back_to_the_next_backend() {
    let (base, upstream) = mock_cc(vec![
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "application/json",
            POOL_EXHAUSTED,
        ),
        (StatusCode::OK, "application/json", ANTHROPIC_REPLY),
    ])
    .await;
    let (_root, state) = cc_state(&base, "test-key");
    let (status, body) = post_json(state, "/v1/chat/completions", user_chat("claude-opus-5")).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let sent = upstream.bodies();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0]["model"], "claude-opus-5");
    assert_eq!(sent[1]["model"], "claude-fable-5");
}

#[tokio::test]
async fn client_errors_pass_through_without_burning_the_fallback_chain() {
    let (base, upstream) = mock_cc(vec![(
        StatusCode::UNAUTHORIZED,
        "application/json",
        r#"{"error":{"message":"Invalid token"}}"#,
    )])
    .await;
    let (_root, state) = cc_state(&base, "wrong-key");
    let (status, body) = post_json(state, "/v1/chat/completions", user_chat("claude-opus-5")).await;

    // key 无效重试两次只会多两个 401，同时掩盖真正的原因。
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.contains("Invalid token"), "{body}");
    assert_eq!(upstream.bodies().len(), 1);
}

#[tokio::test]
async fn every_backend_exhausted_stops_at_the_end_of_the_chain() {
    let script = vec![
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "application/json",
            POOL_EXHAUSTED,
        );
        CC_BACKENDS.len()
    ];
    let (base, upstream) = mock_cc(script).await;
    let (_root, state) = cc_state(&base, "test-key");
    let (status, body) = post_json(state, "/v1/chat/completions", user_chat("claude-opus-5")).await;

    // 链尾不再重试，最后一次的上游状态原样透出。
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(upstream.bodies().len(), CC_BACKENDS.len());
}

#[tokio::test]
async fn non_claude_model_names_start_from_a_real_backend() {
    let (base, upstream) = mock_cc(vec![(StatusCode::OK, "application/json", ANTHROPIC_REPLY)]).await;
    let (_root, state) = cc_state(&base, "test-key");
    // 客户端默认的 gpt-4o 会被归一成 gpt-5.6-sol，上游不认，白跑一轮还占一次容器。
    let (status, body) = post_json(state, "/v1/chat/completions", user_chat("gpt-4o")).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(upstream.bodies()[0]["model"], "claude-opus-5");
}

#[tokio::test]
async fn streaming_reports_the_backend_actually_served_and_carries_usage() {
    let events = concat!(
        "event: message_start\n",
        r#"data: {"type":"message_start","message":{"model":"claude-fable-5","usage":{"input_tokens":10}}}"#,
        "\n\nevent: content_block_delta\n",
        r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#,
        "\n\nevent: message_delta\n",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":10,"output_tokens":3,"cache_read_input_tokens":5}}"#,
        "\n\nevent: message_stop\n",
        r#"data: {"type":"message_stop"}"#,
        "\n\n",
    );
    let (base, upstream) = mock_cc(vec![(StatusCode::OK, "text/event-stream", events)]).await;
    let (_root, state) = cc_state(&base, "test-key");
    let mut request = user_chat("claude-opus-5");
    request["stream"] = json!(true);
    let (status, body) = post_json(state, "/v1/chat/completions", request).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(upstream.bodies()[0]["stream"], true);
    let chunks = sse_events(&body);
    assert_eq!(chunks.len(), 2, "{body}");
    assert_eq!(chunks[0]["choices"][0]["delta"]["content"], "Hello");
    // message_start 之后的每个 chunk 都要报真实后端名。
    assert_eq!(chunks[0]["model"], "claude-fable-5");
    assert_eq!(chunks[1]["choices"][0]["finish_reason"], "stop");
    assert_eq!(chunks[1]["usage"]["prompt_tokens"], 15);
    assert_eq!(chunks[1]["usage"]["completion_tokens"], 3);
    assert_eq!(chunks[1]["usage"]["prompt_tokens_details"]["cached_tokens"], 5);
    assert_eq!(body.matches("data: [DONE]").count(), 1, "{body}");
}

#[tokio::test]
async fn missing_key_is_reported_before_touching_upstream() {
    let (base, upstream) = mock_cc(vec![(StatusCode::OK, "application/json", ANTHROPIC_REPLY)]).await;
    let (_root, state) = cc_state(&base, "");
    let (status, body) = post_json(state, "/v1/chat/completions", user_chat("claude-opus-5")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(body.contains("authentication_error"), "{body}");
    assert!(upstream.bodies().is_empty());
}

#[tokio::test]
async fn responses_route_is_explicitly_unsupported_on_cc() {
    let (base, upstream) = mock_cc(vec![(StatusCode::OK, "application/json", ANTHROPIC_REPLY)]).await;
    let (_root, state) = cc_state(&base, "test-key");
    // cc 没有 Responses 端点，静默按 OpenAI 协议发过去只会换回费解的上游 4xx。
    let (status, body) = post_json(state, "/v1/responses", json!({"input":"hi"})).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("/v1/chat/completions"), "{body}");
    assert!(upstream.bodies().is_empty());
}

#[tokio::test]
async fn models_and_health_describe_the_cc_transport() {
    let (base, _upstream) = mock_cc(Vec::new()).await;
    let (_root, state) = cc_state(&base, "test-key");

    let (status, body) = send(
        state.clone(),
        Request::get("/v1/models").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let listed: Vec<String> = serde_json::from_str::<Value>(&body).unwrap()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap().to_string())
        .collect();
    // 报 gpt-* 会让客户端选到必然被上游改换的模型。
    assert_eq!(listed, CC_BACKENDS);

    let (status, body) = send(
        state.clone(),
        Request::get("/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let health: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(health["transport"], "cc_anthropic");
    assert_eq!(health["upstream_mode"], "direct_anthropic");

    // cc 没有 sidecar，HEALTHCHECK 探 /ready 必须恒为就绪，否则容器永远 unhealthy。
    let (status, body) = send(state, Request::get("/ready").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["status"], "ready");
}
