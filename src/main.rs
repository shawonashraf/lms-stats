use std::future::{Future, pending};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lms_stats::{AppState, backup, db, router};

#[cfg(windows)]
mod service;

pub(crate) const DEFAULT_UPSTREAM: &str =
    "http://192.168.0.166:1234,http://192.168.0.163:1234";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// `LMS_BACKUP_DIR` if set (set but empty disables backups), else `default`.
fn backup_dir(default: Option<PathBuf>) -> Option<PathBuf> {
    match std::env::var("LMS_BACKUP_DIR") {
        Ok(d) if d.is_empty() => None,
        Ok(d) => Some(PathBuf::from(d)),
        Err(_) => default,
    }
}

/// `Documents/lms-stats` in the home directory of the user running the binary.
fn user_backup_dir() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(home).join("Documents").join("lms-stats"))
}

fn log_stderr(msg: &str) {
    eprintln!("lms-stats: {msg}");
}

/// Bind, build the app and serve until `shutdown` resolves. With a
/// `backup_dir`, also takes weekly snapshots of the database there.
async fn serve(
    upstream: String,
    listen: String,
    db_path: String,
    backup_dir: Option<PathBuf>,
    log: fn(&str),
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
    log(&format!("listening on {listen}, upstream {upstream}, db {db_path}"));
    match backup_dir {
        Some(dir) => {
            log(&format!("weekly backups to {}, keeping {}", dir.display(), backup::KEEP));
            tokio::spawn(backup::run(db_path, dir, log));
        }
        None => log("backups disabled"),
    }
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
    let backup_dir = backup_dir(user_backup_dir());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let server = serve(upstream, listen, db_path, backup_dir, log_stderr, pending());
    if let Err(e) = runtime.block_on(server) {
        panic!("{e}");
    }
}
