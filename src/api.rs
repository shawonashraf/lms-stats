use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{
        Html, Json,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{AppState, db, live};

type ApiError = (StatusCode, String);

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(|| async { axum::response::Redirect::to("/dashboard") }))
        .route("/dashboard", get(|| async { Html(include_str!("../static/dashboard.html")) }))
        .route("/api/requests", get(requests))
        .route("/api/aggregate", get(aggregate))
        .route("/api/models", get(models))
        .route("/api/active", get(active))
        .route("/api/events", get(events))
}

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// An empty `model=` from a `<select>` means "all models".
fn model_filter(m: &Option<String>) -> Option<&str> {
    m.as_deref().filter(|s| !s.is_empty())
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    model: Option<String>,
}

fn default_limit() -> i64 {
    50
}

async fn requests(State(s): State<Arc<AppState>>, Query(q): Query<ListQuery>) -> Result<Json<Vec<db::Row>>, ApiError> {
    let conn = s.db.lock().map_err(internal)?;
    db::list(&conn, q.limit.clamp(1, 500), q.offset.max(0), model_filter(&q.model)).map(Json).map_err(internal)
}

#[derive(Deserialize)]
pub struct AggregateQuery {
    #[serde(default = "default_bucket")]
    bucket: String,
    #[serde(default)]
    from: i64,
    #[serde(default = "far_future")]
    to: i64,
    model: Option<String>,
}

fn default_bucket() -> String {
    "day".into()
}

fn far_future() -> i64 {
    i64::MAX
}

#[derive(Serialize)]
pub struct AggregateResponse {
    totals: db::Totals,
    buckets: Vec<db::Bucket>,
}

async fn aggregate(
    State(s): State<Arc<AppState>>,
    Query(q): Query<AggregateQuery>,
) -> Result<Json<AggregateResponse>, ApiError> {
    let bucket = db::BucketSize::parse(&q.bucket)
        .ok_or((StatusCode::BAD_REQUEST, "bucket must be one of day, week, month, year".to_string()))?;
    let conn = s.db.lock().map_err(internal)?;
    let (totals, buckets) = db::aggregate(&conn, bucket, q.from, q.to, model_filter(&q.model)).map_err(internal)?;
    Ok(Json(AggregateResponse { totals, buckets }))
}

async fn models(State(s): State<Arc<AppState>>) -> Result<Json<Vec<String>>, ApiError> {
    let conn = s.db.lock().map_err(internal)?;
    db::models(&conn).map(Json).map_err(internal)
}

async fn active(State(s): State<Arc<AppState>>) -> Json<live::Snapshot> {
    Json(s.live.snapshot())
}

/// Server-sent events: a snapshot immediately, on every request start/finish,
/// and once a second (so elapsed time and chunk counts tick while streaming).
async fn events(State(s): State<Arc<AppState>>) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let rx = s.live.subscribe();
    let ticker = tokio::time::interval(Duration::from_secs(1));
    let stream = futures_util::stream::unfold((s, rx, ticker), |(s, mut rx, mut ticker)| async move {
        tokio::select! {
            r = rx.recv() => {
                if matches!(r, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                    return None;
                }
            }
            _ = ticker.tick() => {}
        }
        let data = serde_json::to_string(&s.live.snapshot()).unwrap_or_else(|_| "{}".into());
        Some((Ok(Event::default().data(data)), (s, rx, ticker)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
