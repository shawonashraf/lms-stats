//! Load and unload models through LM Studio's native REST API (0.4 and later):
//! `GET /api/v1/models`, `POST /api/v1/models/load`, `POST /api/v1/models/unload`.
//! Nothing here is recorded.

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

/// LM Studio's model list passed through as is (`key`, `display_name`,
/// `loaded_instances`, ...), or an `error` string when it is down or too old
/// for the v1 API.
async fn models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let u = &state.upstream;
    let r = async {
        let resp = state.client.get(format!("{u}/api/v1/models")).timeout(LIST_TIMEOUT).send().await?.error_for_status()?;
        resp.json::<Value>().await
    }
    .await;
    Json(match r {
        Ok(v) => json!({"url": u, "models": v["models"].as_array().cloned().unwrap_or_default(), "error": null}),
        Err(e) => json!({"url": u, "models": [], "error": format!("{u}: {}", proxy::error_chain(&e))}),
    })
}

#[derive(Deserialize)]
struct Load {
    model: String,
    #[serde(default)]
    context_length: Option<u64>,
}

async fn load(State(state): State<Arc<AppState>>, Json(req): Json<Load>) -> Response {
    let mut body = json!({ "model": req.model });
    if let Some(n) = req.context_length {
        body["context_length"] = n.into();
    }
    forward(&state, "/api/v1/models/load", body, LOAD_TIMEOUT).await
}

#[derive(Deserialize)]
struct Unload {
    instance_id: String,
}

async fn unload(State(state): State<Arc<AppState>>, Json(req): Json<Unload>) -> Response {
    forward(&state, "/api/v1/models/unload", json!({ "instance_id": req.instance_id }), LOAD_TIMEOUT).await
}

/// POST `body` to LM Studio and hand back its status and body untouched, so
/// its own error messages reach the page; 502 if it cannot be reached.
async fn forward(state: &AppState, path: &str, body: Value, timeout: Duration) -> Response {
    let upstream = &state.upstream;
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
