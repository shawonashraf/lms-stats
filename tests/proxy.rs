use std::sync::{Arc, Mutex};

use axum::{Router, body::Body, extract::Request, response::Response, routing::post};
use lms_stats::{AppState, db, router};

const STREAM_BODY: &str = "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"id\":\"x\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"total_tokens\":16,\"completion_tokens_details\":{\"reasoning_tokens\":3}}}\n\n\
data: [DONE]\n\n";

/// The same stream as STREAM_BODY, split into pieces the "slow" mock sends 150 ms apart.
const SLOW_PIECES: [&str; 3] = [
    "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"x\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"total_tokens\":16,\"completion_tokens_details\":{\"reasoning_tokens\":3}}}\n\ndata: [DONE]\n\n",
];

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
                    if v["model"] == "slow" {
                        let slow = futures_util::stream::unfold(0usize, |i| async move {
                            if i >= SLOW_PIECES.len() {
                                return None;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                            Some((Ok::<_, std::io::Error>(bytes::Bytes::from(SLOW_PIECES[i])), i + 1))
                        });
                        return Response::builder()
                            .header("content-type", "text/event-stream")
                            .body(Body::from_stream(slow))
                            .unwrap();
                    }
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
        .route("/v1/models", axum::routing::get(|| async { r#"{"data":[]}"# }))
        .route(
            "/v1/completions",
            post(|| async {
                Response::builder()
                    .status(400)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"error":{"message":"bad request"}}"#))
                    .unwrap()
            }),
        );
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
        live: lms_stats::live::Live::new(),
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
    assert_eq!(r.ttft_ms, None, "no first-token time for a buffered response");
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
    let ttft = r.ttft_ms.expect("streamed response records time to first token");
    assert!(ttft <= r.duration_ms, "ttft {ttft} > duration {}", r.duration_ms);
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
    let lower = msg.to_lowercase();
    assert!(lower.contains("refused") || lower.contains("connect"), "{msg}");
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

#[tokio::test]
async fn streaming_request_with_json_error_upstream_is_forwarded_and_recorded() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen).await).await;
    let resp = reqwest::Client::new()
        .post(format!("{proxy}/v1/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m","prompt":"x","stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body = resp.text().await.unwrap();
    assert_eq!(body, r#"{"error":{"message":"bad request"}}"#);

    let rows = wait_for_rows(&state, 1).await;
    let r = &rows[0];
    assert_eq!((r.status, r.total_tokens, r.stream), (400, 0, true));
}

#[tokio::test]
async fn api_lists_and_aggregates() {
    let (proxy, state) = start("http://127.0.0.1:1".to_string()).await;
    {
        let conn = state.db.lock().unwrap();
        for (ts, model, p, c) in [(1_790_337_600, "a", 10, 5), (1_790_424_000, "b", 20, 8)] {
            db::insert(
                &conn,
                &db::Row {
                    id: 0,
                    ts,
                    endpoint: "/v1/chat/completions".into(),
                    model: model.into(),
                    prompt_tokens: p,
                    completion_tokens: c,
                    reasoning_tokens: 0,
                    total_tokens: p + c,
                    stream: false,
                    status: 200,
                    duration_ms: 1,
                    ttft_ms: None,
                },
            )
            .unwrap();
        }
    }

    let rows: Vec<serde_json::Value> =
        reqwest::get(format!("{proxy}/api/requests?limit=1")).await.unwrap().json().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["model"], "b");

    let agg: serde_json::Value =
        reqwest::get(format!("{proxy}/api/aggregate?bucket=day")).await.unwrap().json().await.unwrap();
    assert_eq!(agg["totals"]["requests"], 2);
    assert_eq!(agg["totals"]["total"], 43);
    assert_eq!(agg["buckets"].as_array().unwrap().len(), 2);

    let agg_a: serde_json::Value =
        reqwest::get(format!("{proxy}/api/aggregate?bucket=month&model=a")).await.unwrap().json().await.unwrap();
    assert_eq!(agg_a["totals"]["requests"], 1);

    let bad = reqwest::get(format!("{proxy}/api/aggregate?bucket=hour")).await.unwrap();
    assert_eq!(bad.status(), 400);

    let models: Vec<String> = reqwest::get(format!("{proxy}/api/models")).await.unwrap().json().await.unwrap();
    assert_eq!(models, vec!["a", "b"]);

    let html = reqwest::get(format!("{proxy}/dashboard")).await.unwrap();
    assert_eq!(html.status(), 200);
    assert!(html.headers()["content-type"].to_str().unwrap().starts_with("text/html"));

    let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let root = no_redirect.get(format!("{proxy}/")).send().await.unwrap();
    assert!(root.status().is_redirection(), "{}", root.status());
    assert_eq!(root.headers()["location"], "/dashboard");
}

#[tokio::test]
async fn in_flight_stream_is_visible_in_active_and_events_then_recorded() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen).await).await;
    let client = reqwest::Client::new();

    // Before anything runs: no active requests, nothing completed.
    let snap: serde_json::Value = reqwest::get(format!("{proxy}/api/active")).await.unwrap().json().await.unwrap();
    assert_eq!(snap["active"].as_array().unwrap().len(), 0);
    assert_eq!(snap["completed"], 0);

    // Fire a slow streamed request in the background.
    let proxy2 = proxy.clone();
    let client2 = client.clone();
    let req = tokio::spawn(async move {
        client2
            .post(format!("{proxy2}/v1/chat/completions"))
            .header("content-type", "application/json")
            .body(r#"{"model":"slow","messages":[],"stream":true}"#)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });

    // While it streams, /api/active shows it with a growing chunk count.
    let mut seen_progress = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        let snap: serde_json::Value = reqwest::get(format!("{proxy}/api/active")).await.unwrap().json().await.unwrap();
        let active = snap["active"].as_array().unwrap();
        if active.len() == 1 && active[0]["chunks"].as_u64().unwrap() >= 1 {
            assert_eq!(active[0]["model"], "slow");
            assert_eq!(active[0]["stream"], true);
            assert!(active[0]["elapsed_ms"].as_u64().unwrap() > 0);
            seen_progress = true;
            break;
        }
    }
    assert!(seen_progress, "never saw the in-flight request with chunks >= 1");

    // /api/events is an SSE stream whose first event is a JSON snapshot.
    let mut ev = client.get(format!("{proxy}/api/events")).send().await.unwrap();
    assert!(ev.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let first = ev.chunk().await.unwrap().unwrap();
    let first = String::from_utf8(first.to_vec()).unwrap();
    assert!(first.starts_with("data: "), "{first}");
    let payload: serde_json::Value = serde_json::from_str(first.trim_start_matches("data: ").trim()).unwrap();
    assert!(payload.get("active").is_some() && payload.get("completed").is_some());

    // After it finishes: body intact (usage chunk stripped), row recorded, active empty, completed bumped.
    let body = req.await.unwrap();
    assert!(body.contains("\"content\":\"hi\"") && !body.contains("usage"));
    let rows = wait_for_rows(&state, 1).await;
    assert_eq!((rows[0].total_tokens, rows[0].stream, rows[0].status), (16, true, 200));
    // The slow mock waits 150 ms before its first chunk, so ttft reflects real waiting.
    let ttft = rows[0].ttft_ms.unwrap();
    assert!((140..=rows[0].duration_ms).contains(&ttft), "ttft {ttft} ms, duration {} ms", rows[0].duration_ms);
    let snap: serde_json::Value = reqwest::get(format!("{proxy}/api/active")).await.unwrap().json().await.unwrap();
    assert_eq!(snap["active"].as_array().unwrap().len(), 0);
    assert_eq!(snap["completed"], 1);
}
