use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::AppState;

pub async fn handler(State(_state): State<Arc<AppState>>, _req: Request) -> Response {
    (StatusCode::NOT_IMPLEMENTED, "proxy not implemented yet").into_response()
}
