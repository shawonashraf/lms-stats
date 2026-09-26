use std::sync::{Arc, Mutex};

use lms_stats::{AppState, db, router};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    let upstream = env_or("LMS_UPSTREAM", "http://192.168.0.166:1234");
    let listen = env_or("LMS_LISTEN", "0.0.0.0:1235");
    let db_path = env_or("LMS_DB", "./lms-stats.db");

    let conn = db::open(&db_path).unwrap_or_else(|e| panic!("open {db_path}: {e}"));
    let state = Arc::new(AppState {
        upstream: upstream.trim_end_matches('/').to_string(),
        client: reqwest::Client::new(),
        db: Mutex::new(conn),
        live: lms_stats::live::Live::new(),
    });

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| panic!("bind {listen}: {e}"));
    eprintln!("lms-stats: listening on {listen}, upstream {upstream}, db {db_path}");
    axum::serve(listener, router(state)).await.unwrap();
}
