use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, Json},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{AppState, db};

type ApiError = (StatusCode, String);

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/dashboard", get(|| async { Html(include_str!("../static/dashboard.html")) }))
        .route("/api/requests", get(requests))
        .route("/api/aggregate", get(aggregate))
        .route("/api/models", get(models))
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
