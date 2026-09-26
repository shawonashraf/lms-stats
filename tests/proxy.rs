use std::sync::{Arc, Mutex};

use axum::{Router, body::Body, extract::Request, response::Response, routing::post};
use lms_stats::{AppState, db, router};

const STREAM_BODY: &str = "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"id\":\"x\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"total_tokens\":16,\"completion_tokens_details\":{\"reasoning_tokens\":3}}}\n\n\
data: [DONE]\n\n";

const JSON_BODY: &str = r#"{"id":"y","choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#;

/// Mock LM Studio: records the last request body, answers streaming or JSON based on `stream`.
async fn mock_upstream(seen: Arc<Mutex<Vec<String>>>) -> String {
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(move |req: Request| {
                let seen = seen.clone();
                async move {
                    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX).await.unwrap();
                    let text = String::from_utf8(bytes.to_vec()).unwrap();
                    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                    seen.lock().unwrap().push(text);
                    if v["stream"].as_bool().unwrap_or(false) {
                        Response::builder()
                            .header("content-type", "text/event-stream")
                            .body(Body::from(STREAM_BODY))
                            .unwrap()
                    } else {
                        Response::builder()
                            .header("content-type", "application/json")
                            .body(Body::from(JSON_BODY))
                            .unwrap()
                    }
                }
            }),
        )
        .route("/v1/models", axum::routing::get(|| async { r#"{"data":[]}"# }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

async fn start(upstream: String) -> (String, Arc<AppState>) {
    let state = Arc::new(AppState {
        upstream,
        client: reqwest::Client::new(),
        db: Mutex::new(db::open_memory().unwrap()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}

async fn wait_for_rows(state: &AppState, n: usize) -> Vec<db::Row> {
    for _ in 0..50 {
        let rows = db::list(&state.db.lock().unwrap(), 50, 0, None).unwrap();
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected {n} rows");
}

#[tokio::test]
async fn non_streaming_request_is_forwarded_and_recorded() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen.clone()).await).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m1","messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), JSON_BODY);

    let rows = wait_for_rows(&state, 1).await;
    let r = &rows[0];
    assert_eq!((r.model.as_str(), r.endpoint.as_str(), r.stream, r.status), ("m1", "/v1/chat/completions", false, 200));
    assert_eq!((r.prompt_tokens, r.completion_tokens, r.reasoning_tokens, r.total_tokens), (5, 2, 0, 7));
}

#[tokio::test]
async fn streaming_request_injects_include_usage_and_strips_usage_chunk() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen.clone()).await).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m2","messages":[],"stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(!body.contains("usage"), "usage chunk leaked to client: {body}");
    assert!(body.contains("\"content\":\"hi\""));
    assert!(body.ends_with("data: [DONE]\n\n"));

    let sent: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(sent["stream_options"]["include_usage"], serde_json::Value::Bool(true));

    let rows = wait_for_rows(&state, 1).await;
    let r = &rows[0];
    assert!(r.stream);
    assert_eq!((r.prompt_tokens, r.completion_tokens, r.reasoning_tokens, r.total_tokens), (12, 4, 3, 16));
}

#[tokio::test]
async fn streaming_client_that_asked_for_usage_keeps_the_chunk() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen.clone()).await).await;
    let body = reqwest::Client::new()
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m3","messages":[],"stream":true,"stream_options":{"include_usage":true}}"#)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, STREAM_BODY);
    let rows = wait_for_rows(&state, 1).await;
    assert_eq!(rows[0].total_tokens, 16);
}

#[tokio::test]
async fn uncounted_paths_pass_through_without_rows() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen).await).await;
    let body = reqwest::get(format!("{proxy}/v1/models")).await.unwrap().text().await.unwrap();
    assert_eq!(body, r#"{"data":[]}"#);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(db::list(&state.db.lock().unwrap(), 50, 0, None).unwrap().is_empty());
}

#[tokio::test]
async fn unreachable_upstream_returns_502_and_records_failure() {
    let (proxy, state) = start("http://127.0.0.1:1".to_string()).await;
    let resp = reqwest::Client::new()
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m","messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    let body: serde_json::Value = resp.json().await.unwrap();
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("LM Studio at http://127.0.0.1:1 is unavailable"), "{msg}");
    assert_eq!(body["error"]["type"], "upstream_unavailable");

    let rows = wait_for_rows(&state, 1).await;
    assert_eq!((rows[0].status, rows[0].total_tokens, rows[0].model.as_str()), (502, 0, "m"));
}

#[tokio::test]
async fn unreachable_upstream_on_uncounted_path_records_nothing() {
    let (proxy, state) = start("http://127.0.0.1:1".to_string()).await;
    let resp = reqwest::get(format!("{proxy}/v1/models")).await.unwrap();
    assert_eq!(resp.status(), 502);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(db::list(&state.db.lock().unwrap(), 50, 0, None).unwrap().is_empty());
}
