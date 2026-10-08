pub mod api;
pub mod backup;
pub mod db;
pub mod live;
pub mod proxy;
pub mod usage;

use std::sync::{Arc, Mutex};

pub struct AppState {
    /// Base URLs of the LM Studio instances without trailing slash, e.g.
    /// `http://192.168.0.166:1234`. Never empty; the first is the default.
    pub upstreams: Vec<String>,
    pub client: reqwest::Client,
    pub db: Mutex<rusqlite::Connection>,
    pub live: live::Live,
}

pub fn router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .merge(api::router())
        .fallback(proxy::handler)
        .with_state(state)
}
