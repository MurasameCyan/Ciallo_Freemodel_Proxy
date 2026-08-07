use axum::{
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use freemodel_workbuddy_proxy::{
    config::Config,
    server::{AppState, router},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{collections::HashMap, net::SocketAddr, path::Path};
use tempfile::{TempDir, tempdir};
use tower::ServiceExt;

const PROXY_KEY: &str = "proxy-key-for-the-setup-page";
const OLD_KEY: &str = "fe_oa_old_key_from_the_environment";
const NEW_KEY: &str = "fe_oa_new_key_typed_into_the_page";

fn env(root: &Path) -> HashMap<String, String> {
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    HashMap::from([
        ("HOME".into(), root.to_string_lossy().to_string()),
        (
            "FREEMODEL_BASE_URL".into(),
            "https://cc.freemodel.dev/v1".into(),
        ),
        ("FREEMODEL_API_KEY".into(), OLD_KEY.into()),
        ("PROXY_API_KEY".into(), PROXY_KEY.into()),
        (
            "PROXY_DEFAULT_PROJECT".into(),
            project.to_string_lossy().to_string(),
        ),
        (
            "PROXY_SESSION_STORE".into(),
            root.join("sessions.json").to_string_lossy().to_string(),
        ),
        (
            "PROXY_RUNTIME_DIR".into(),
            root.join("runtime").to_string_lossy().to_string(),
        ),
    ])
}

fn setup_state() -> (TempDir, AppState) {
    let root = tempdir().unwrap();
    let config = Config::load_with_env(root.path(), &env(root.path())).unwrap();
    let state = AppState::new(config).unwrap();
    (root, state)
}

async fn send(state: &AppState, request: Request<Body>) -> (StatusCode, String, String) {
    let response = router(state.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))))
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, String::from_utf8(bytes.to_vec()).unwrap())
}

fn save_request(key: &str, bearer: Option<&str>) -> Request<Body> {
    let mut request = Request::post("/setup/key").header("content-type", "application/json");
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    request
        .body(Body::from(json!({ "key": key }).to_string()))
        .unwrap()
}

#[tokio::test]
async fn setup_page_is_served_as_html_and_advertised_by_health() {
    let (_root, state) = setup_state();

    let (status, content_type, body) =
        send(&state, Request::get("/setup").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/html"), "{content_type}");
    assert!(body.contains("<form id=\"form\""), "{body}");

    // 页面得能被找到：默认落地页是 `/` 的那串 JSON，用户看不到任何入口。
    let (_, _, health) = send(&state, Request::get("/").body(Body::empty()).unwrap()).await;
    let health: Value = serde_json::from_str(&health).unwrap();
    assert_eq!(health["setup_url"], "/setup");
}

#[tokio::test]
async fn saving_a_key_takes_effect_immediately_and_survives_a_restart() {
    let (root, state) = setup_state();

    // 没有 key 就能改上游凭据，等于把额度送给同网段任何人。
    let (status, _, _) = send(&state, save_request(NEW_KEY, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = send(&state, save_request(NEW_KEY, Some("wrong-key"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(state.upstream_key(), OLD_KEY);

    // key 会被拼进 Authorization header，换行必须在入口就挡掉。
    for bad in ["", "   ", "fe_key\nX-Injected: 1", "fe_key with space", "fe_ké"] {
        let (status, _, body) = send(&state, save_request(bad, Some(PROXY_KEY))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?} → {body}");
    }
    assert_eq!(state.upstream_key(), OLD_KEY);

    let (status, _, body) = send(&state, save_request(NEW_KEY, Some(PROXY_KEY))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.contains(NEW_KEY), "响应回显了完整 key: {body}");
    let saved: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(saved["freemodel_key"], "fe_o••••page");

    // 立即生效：上游请求读的是这份可写副本，而不是启动时的 config。
    assert_eq!(state.upstream_key(), NEW_KEY);

    let request = Request::get("/setup/key")
        .header("authorization", format!("Bearer {PROXY_KEY}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&state, request).await;
    assert_eq!(status, StatusCode::OK);
    let status_body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(status_body["configured"], true);
    assert_eq!(status_body["freemodel_key"], "fe_o••••page");

    // 也落了盘：用同一份仍指向旧 key 的环境重新加载，config.json 优先级更高，
    // 所以重启后拿到的应当是页面刚存进去的那把。
    let reloaded = Config::load_with_env(root.path(), &env(root.path())).unwrap();
    assert_eq!(reloaded.api_key, NEW_KEY);
}
