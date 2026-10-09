//! Load and unload models through LM Studio's native REST API (0.4 and later):
//! `GET /api/v1/models`, `POST /api/v1/models/load`, `POST /api/v1/models/unload`.
//! The `lms` CLI only manages the LM Studio on the machine it runs on, so the
//! API is the one way to reach a remote upstream. Nothing here is recorded.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AppState, proxy};

/// Listing is cheap; a powered-off upstream must not stall the page.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
/// A large model from a cold disk takes minutes to load.
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/admin/models", get(models))
        .route("/api/admin/load", post(load))
        .route("/api/admin/unload", post(unload))
}

/// Every configured upstream with LM Studio's model list passed through as is
/// (`key`, `display_name`, `loaded_instances`, ...), or an `error` string for
/// one that is down or runs an LM Studio without the v1 API.
async fn models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let state = &*state;
    let fetch = |u: &String| {
        let url = format!("{u}/api/v1/models");
        let u = u.clone();
        async move {
            let r = async {
                let resp = state.client.get(&url).timeout(LIST_TIMEOUT).send().await?.error_for_status()?;
                resp.json::<Value>().await
            }
            .await;
            match r {
                Ok(v) => json!({"url": u, "models": v["models"].as_array().cloned().unwrap_or_default(), "error": null}),
                Err(e) => json!({"url": u, "models": [], "error": format!("{u}: {}", proxy::error_chain(&e))}),
            }
        }
    };
    let upstreams = futures_util::future::join_all(state.upstreams.iter().map(fetch)).await;
    Json(json!({ "upstreams": upstreams }))
}

#[derive(Deserialize)]
struct Load {
    upstream: String,
    model: String,
    #[serde(default)]
    context_length: Option<u64>,
}

async fn load(State(state): State<Arc<AppState>>, Json(req): Json<Load>) -> Response {
    let Some(u) = configured(&state, &req.upstream) else {
        return not_configured();
    };
    let mut body = json!({ "model": req.model });
    if let Some(n) = req.context_length {
        body["context_length"] = n.into();
    }
    forward(&state, u, "/api/v1/models/load", body, LOAD_TIMEOUT).await
}

#[derive(Deserialize)]
struct Unload {
    upstream: String,
    instance_id: String,
}

async fn unload(State(state): State<Arc<AppState>>, Json(req): Json<Unload>) -> Response {
    let Some(u) = configured(&state, &req.upstream) else {
        return not_configured();
    };
    forward(&state, u, "/api/v1/models/unload", json!({ "instance_id": req.instance_id }), LOAD_TIMEOUT).await
}

/// The dashboard may only talk to upstreams given at startup, never to an
/// arbitrary host named in the request.
fn configured<'a>(state: &'a AppState, upstream: &str) -> Option<&'a str> {
    state.upstreams.iter().find(|u| u.as_str() == upstream).map(String::as_str)
}

fn not_configured() -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": "upstream is not configured" }))).into_response()
}

/// POST `body` to `upstream + path` and hand back LM Studio's status and body
/// untouched, so its own error messages reach the page; 502 if it cannot be reached.
async fn forward(state: &AppState, upstream: &str, path: &str, body: Value, timeout: Duration) -> Response {
    let resp = match state.client.post(format!("{upstream}{path}")).timeout(timeout).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("LM Studio at {upstream} is unavailable: {}", proxy::error_chain(&e));
            return (StatusCode::BAD_GATEWAY, Json(json!({ "error": msg }))).into_response();
        }
    };
    let status = resp.status();
    let content_type = resp.headers().get(header::CONTENT_TYPE).cloned();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            let msg = format!("LM Studio at {upstream}: {}", proxy::error_chain(&e));
            return (StatusCode::BAD_GATEWAY, Json(json!({ "error": msg }))).into_response();
        }
    };
    let mut r = Response::new(axum::body::Body::from(bytes));
    *r.status_mut() = status;
    if let Some(ct) = content_type {
        r.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    r
}
