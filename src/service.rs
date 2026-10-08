//! Windows service entry point, the equivalent of `lms-stats.service`.
//!
//! The binary is a normal console app, but when SCM starts it with the
//! `run-as-service` argument it talks to the Service Control Manager instead:
//! reports start/stop transitions, stops gracefully on ServiceControl::Stop
//! (with a grace period so a stuck SSE connection cannot block shutdown) and
//! exits non-zero on failure so the configured recovery actions restart it.

use std::ffi::OsString;
use std::time::Duration;

use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;
use windows_service::{Result, define_windows_service};

use crate::{DEFAULT_UPSTREAM, backup_dir, env_or, serve};

const SERVICE_NAME: &str = "lms-stats";
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

pub fn run() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = run_service() {
        log(&format!("service error: {e:?}"));
        std::process::exit(1);
    }
}

fn data_dir() -> std::path::PathBuf {
    let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".to_string());
    std::path::Path::new(&base).join("lms-stats")
}

fn default_db_path() -> String {
    data_dir().join("lms-stats.db").to_string_lossy().into_owned()
}

fn log(msg: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(data_dir());
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = data_dir().join("service.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

fn run_service() -> Result<()> {
    let upstream = env_or("LMS_UPSTREAM", DEFAULT_UPSTREAM);
    let listen = env_or("LMS_LISTEN", "0.0.0.0:1235");
    let db_path = env_or("LMS_DB", &default_db_path());
    // The service account's Documents folder is not the user's, so there is
    // no default here; install-service.ps1 sets LMS_BACKUP_DIR.
    let backup_dir = backup_dir(None);

    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    log("starting");

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(());
    let (done_tx, done_rx) =
        std::sync::mpsc::channel::<std::result::Result<(), String>>();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    let status = |state, exit_code, checkpoint| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code,
        wait_hint: Duration::from_secs(5),
        process_id: None,
        checkpoint,
    };
    status_handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceExitCode::Win32(0),
        1,
    ))?;

    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = done_tx.send(Err(format!("build tokio runtime: {e}")));
                return;
            }
        };
        let result = runtime.block_on(async move {
            let mut shutdown_rx = stop_rx.clone();
            let server = serve(
                upstream,
                listen,
                db_path,
                backup_dir,
                log,
                async move {
                    let _ = shutdown_rx.changed().await;
                },
            );
            tokio::pin!(server);
            let mut stop_watcher = stop_rx;
            tokio::select! {
                r = &mut server => r,
                _ = stop_watcher.changed() => {
                    // Stop requested: the server is now shutting down
                    // gracefully; if it does not finish within the grace
                    // period, abandon the wait and let the process exit.
                    match tokio::time::timeout(SHUTDOWN_GRACE, &mut server).await {
                        Ok(r) => r,
                        Err(_) => Ok(()),
                    }
                }
            }
        });
        let _ = done_tx.send(result);
    });

    status_handle.set_service_status(status(
        ServiceState::Running,
        ServiceExitCode::Win32(0),
        0,
    ))?;

    let (exit_code, failed) = match done_rx.recv() {
        Ok(Ok(())) => (ServiceExitCode::Win32(0), false),
        Ok(Err(e)) => {
            log(&format!("stopped with error: {e}"));
            (ServiceExitCode::ServiceSpecific(1), true)
        }
        Err(_) => {
            log("server thread died without reporting");
            (ServiceExitCode::ServiceSpecific(1), true)
        }
    };
    if !failed {
        log("stopped");
    }
    status_handle.set_service_status(status(ServiceState::Stopped, exit_code, 0))?;

    // A non-zero exit code makes the SCM treat this as a service failure, so
    // the configured recovery actions restart the process.
    if failed {
        std::process::exit(1);
    }
    Ok(())
}
