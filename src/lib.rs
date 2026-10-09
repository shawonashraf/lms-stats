pub mod admin;
pub mod api;
pub mod backup;
pub mod db;
pub mod live;
pub mod proxy;
pub mod usage;

use std::sync::{Arc, Mutex};

pub struct AppState {
    /// Base URL of LM Studio without trailing slash, e.g. `http://localhost:1234`.
    /// One instance only: devices joined through LM Link are federated by LM
    /// Studio itself behind this URL.
    pub upstream: String,
    pub client: reqwest::Client,
    pub db: Mutex<rusqlite::Connection>,
    pub live: live::Live,
}

pub fn router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .merge(api::router())
        .merge(admin::router())
        .fallback(proxy::handler)
        .with_state(state)
}
