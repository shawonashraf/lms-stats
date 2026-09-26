use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::Value;

use crate::{
    AppState, db,
    usage::{self, SseTap, Usage},
};

const COUNTED: &[&str] = &["/v1/chat/completions", "/v1/completions", "/v1/embeddings"];

/// Cap request body buffering so a runaway client can't exhaust memory; 64 MiB
/// leaves room for base64-encoded vision payloads.
const MAX_BODY: usize = 64 << 20;

/// Headers that must not be copied between the two hops. `content-length` is
/// recomputed; `accept-encoding` is dropped so upstream never compresses a body
/// we need to read.
const SKIP_HEADERS: &[&str] = &[
    "host",
    "connection",
    "transfer-encoding",
    "content-length",
    "keep-alive",
    "upgrade",
    "proxy-connection",
    "accept-encoding",
];

/// What we learned from a counted request body.
struct Counted {
    model: String,
    stream: bool,
    /// The client itself asked for `stream_options.include_usage`, so the usage
    /// chunk is forwarded rather than dropped.
    client_wanted_usage: bool,
    /// Handle in the in-flight tracker; released by `record`.
    live_id: u64,
}

pub async fn handler(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let started = Instant::now();
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);

    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let path_and_query = parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| path.clone());
    let mut body = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b.to_vec(),
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let mut counted = None;
    if parts.method == Method::POST
        && COUNTED.contains(&path.as_str())
        && let Ok(mut v) = serde_json::from_slice::<Value>(&body)
        && v.is_object()
    {
        let model = v.get("model").and_then(Value::as_str).unwrap_or("").to_string();
        let stream = v.get("stream").and_then(Value::as_bool).unwrap_or(false);
        let mut client_wanted_usage = true;
        if stream {
            client_wanted_usage = v.pointer("/stream_options/include_usage").and_then(Value::as_bool).unwrap_or(false);
            if !client_wanted_usage {
                if !v["stream_options"].is_object() {
                    v["stream_options"] = serde_json::json!({});
                }
                v["stream_options"]["include_usage"] = Value::Bool(true);
                body = serde_json::to_vec(&v).expect("re-serialize json");
            }
        }
        let live_id = state.live.start(ts, &path, &model, stream);
        counted = Some(Counted { model, stream, client_wanted_usage, live_id });
    }

    let mut upstream = state.client.request(parts.method.clone(), format!("{}{}", state.upstream, path_and_query));
    for (k, v) in &parts.headers {
        if !SKIP_HEADERS.contains(&k.as_str()) {
            upstream = upstream.header(k, v);
        }
    }
    let resp = match upstream.body(body).send().await {
        Ok(r) => r,
        Err(e) => {
            // Make outages visible on the dashboard: one zero-count row with status 502.
            if let Some(c) = &counted {
                record(&state, ts, &path, c, StatusCode::BAD_GATEWAY, started, None);
            }
            return unavailable(&state.upstream, &e);
        }
    };
    let status = resp.status();
    let mut headers = HeaderMap::new();
    for (k, v) in resp.headers() {
        if !SKIP_HEADERS.contains(&k.as_str()) {
            headers.insert(k.clone(), v.clone());
        }
    }

    let Some(c) = counted else {
        return build(status, headers, Body::from_stream(resp.bytes_stream()));
    };

    if !c.stream {
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                record(&state, ts, &path, &c, StatusCode::BAD_GATEWAY, started, None);
                return (StatusCode::BAD_GATEWAY, format!("upstream body error: {e}")).into_response();
            }
        };
        let found = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| usage::from_json(&v));
        record(&state, ts, &path, &c, status, started, found);
        return build(status, headers, Body::from(bytes));
    }

    // Streaming: forward chunks through the tap as they arrive; record once the
    // upstream stream ends (or the client goes away).
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(16);
    let state2 = state.clone();
    tokio::spawn(async move {
        let mut tap = SseTap::new(!c.client_wanted_usage);
        let mut upstream_body = resp.bytes_stream();
        // Recorded status defaults to the upstream's; overridden below if the
        // stream ends abnormally: 499 (client closed request) if the client
        // went away, 502 (bad gateway) if the upstream stream itself errored.
        let mut outcome = status;
        while let Some(item) = upstream_body.next().await {
            match item {
                Ok(chunk) => {
                    let out = tap.feed(&chunk);
                    state2.live.progress(c.live_id, tap.events);
                    if !out.is_empty() && tx.send(Ok(out.into())).await.is_err() {
                        outcome = StatusCode::from_u16(499).unwrap();
                        break; // client disconnected; dropping `upstream_body` aborts upstream
                    }
                }
                Err(e) => {
                    outcome = StatusCode::BAD_GATEWAY;
                    let _ = tx.send(Err(std::io::Error::other(e))).await;
                    break;
                }
            }
        }
        let rest = tap.finish();
        if !rest.is_empty() {
            let _ = tx.send(Ok(rest.into())).await;
        }
        record(&state2, ts, &path, &c, outcome, started, tap.usage);
    });
    let client_body = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
    build(status, headers, Body::from_stream(client_body))
}

/// OpenAI-style error body so SDK clients surface a readable message instead of a bare 502.
fn unavailable(upstream: &str, err: &reqwest::Error) -> Response {
    let mut cause = err.to_string();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(e) = source {
        cause.push_str(": ");
        cause.push_str(&e.to_string());
        source = e.source();
    }
    let body = serde_json::json!({
        "error": {
            "message": format!("LM Studio at {upstream} is unavailable: {cause}"),
            "type": "upstream_unavailable"
        }
    });
    (StatusCode::BAD_GATEWAY, axum::Json(body)).into_response()
}

fn build(status: StatusCode, headers: HeaderMap, body: Body) -> Response {
    let mut r = Response::new(body);
    *r.status_mut() = status;
    *r.headers_mut() = headers;
    r
}

fn record(state: &AppState, ts: i64, endpoint: &str, c: &Counted, status: StatusCode, started: Instant, found: Option<Usage>) {
    let u = found.unwrap_or_default();
    let row = db::Row {
        id: 0,
        ts,
        endpoint: endpoint.to_string(),
        model: c.model.clone(),
        prompt_tokens: u.prompt,
        completion_tokens: u.completion,
        reasoning_tokens: u.reasoning,
        total_tokens: u.total,
        stream: c.stream,
        status: status.as_u16(),
        duration_ms: started.elapsed().as_millis() as i64,
    };
    // ponytail: std Mutex held for one INSERT; move to a writer task if the lock ever shows up in profiles.
    let result = match state.db.lock() {
        Ok(conn) => db::insert(&conn, &row).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = result {
        eprintln!("lms-stats: db insert failed: {e}");
    }
    state.live.finish(c.live_id);
}
