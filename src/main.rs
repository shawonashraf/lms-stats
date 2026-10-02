use std::future::{Future, pending};
use std::sync::{Arc, Mutex};

use lms_stats::{AppState, db, router};

#[cfg(windows)]
mod service;

pub(crate) const DEFAULT_UPSTREAM: &str =
    "http://192.168.0.166:1234,http://192.168.0.163:1234";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Bind, build the app and serve until `shutdown` resolves.
async fn serve(
    upstream: String,
    listen: String,
    db_path: String,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let conn = db::open(&db_path).map_err(|e| format!("open {db_path}: {e}"))?;
    let state = Arc::new(AppState {
        upstreams: upstream
            .split(',')
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .collect(),
        client: reqwest::Client::new(),
        db: Mutex::new(conn),
        live: lms_stats::live::Live::new(),
    });

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|e| format!("bind {listen}: {e}"))?;
    eprintln!("lms-stats: listening on {listen}, upstream {upstream}, db {db_path}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| format!("serve: {e}"))
}

fn main() {
    if cfg!(windows) && std::env::args().nth(1).as_deref() == Some("run-as-service") {
        #[cfg(windows)]
        {
            if let Err(e) = service::run() {
                eprintln!("lms-stats: service: {e:?}");
                std::process::exit(1);
            }
        }
        #[cfg(not(windows))]
        unreachable!("run-as-service is only valid on Windows");
        return;
    }

    let upstream = env_or("LMS_UPSTREAM", DEFAULT_UPSTREAM);
    let listen = env_or("LMS_LISTEN", "0.0.0.0:1235");
    let db_path = env_or("LMS_DB", "./lms-stats.db");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    if let Err(e) = runtime.block_on(serve(upstream, listen, db_path, pending())) {
        panic!("{e}");
    }
}
