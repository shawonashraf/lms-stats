//! Weekly snapshots of the database into a backup directory.
//!
//! Every hour the task looks at the newest `lms-stats-YYYY-MM-DD.db` in the
//! directory; when there is none or it is a week old, it writes a new one with
//! `VACUUM INTO` (a consistent copy, safe while the proxy is writing), then
//! deletes all but the newest `KEEP`. Other files in the directory are left
//! alone.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};

pub const KEEP: usize = 5;
const INTERVAL_DAYS: f64 = 7.0;
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
const PREFIX: &str = "lms-stats-";
const SUFFIX: &str = ".db";

/// Check now and then every hour, forever. Failures are logged and retried
/// on the next check.
pub async fn run(db_path: String, dir: PathBuf, log: fn(&str)) {
    let mut tick = tokio::time::interval(CHECK_EVERY);
    loop {
        tick.tick().await;
        let (db, d) = (db_path.clone(), dir.clone());
        match tokio::task::spawn_blocking(move || snapshot_if_due(&db, &d, None)).await {
            Ok(Ok(Some(path))) => log(&format!("backup written to {}", path.display())),
            Ok(Ok(None)) => {}
            Ok(Err(e)) => log(&format!("backup to {} failed: {e}", dir.display())),
            Err(e) => log(&format!("backup task panicked: {e}")),
        }
    }
}

/// Take a snapshot if the newest one is at least a week older than `today`
/// (`YYYY-MM-DD`, local date when `None`), then prune. Returns the path of the
/// new snapshot, if one was taken.
pub fn snapshot_if_due(db_path: &str, dir: &Path, today: Option<&str>) -> Result<Option<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {db_path}: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())?;

    let today: String = match today {
        Some(t) => t.to_string(),
        None => conn
            .query_row("SELECT date('now', 'localtime')", [], |r| r.get(0))
            .map_err(|e| e.to_string())?,
    };
    let due = match snapshots(dir)?.last() {
        None => true,
        Some(newest) => {
            let age: f64 = conn
                .query_row("SELECT julianday(?1) - julianday(?2)", params![today, newest], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            age >= INTERVAL_DAYS
        }
    };

    let mut taken = None;
    if due {
        let dest = dir.join(format!("{PREFIX}{today}{SUFFIX}"));
        // Write under a name the pruning ignores, so a half-written file is
        // never counted as a snapshot.
        let tmp = dir.join(format!("{PREFIX}{today}{SUFFIX}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy()])
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &dest).map_err(|e| format!("rename to {}: {e}", dest.display()))?;
        taken = Some(dest);
    }

    let dates = snapshots(dir)?;
    for date in &dates[..dates.len().saturating_sub(KEEP)] {
        let old = dir.join(format!("{PREFIX}{date}{SUFFIX}"));
        std::fs::remove_file(&old).map_err(|e| format!("delete {}: {e}", old.display()))?;
    }
    Ok(taken)
}

/// Dates of the snapshots in `dir`, oldest first.
fn snapshots(dir: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    let mut dates: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| snapshot_date(&e.file_name().to_string_lossy()).map(str::to_string))
        .collect();
    dates.sort();
    Ok(dates)
}

/// `lms-stats-2026-10-06.db` -> `2026-10-06`; anything else -> None.
fn snapshot_date(name: &str) -> Option<&str> {
    let date = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let ok = date.len() == 10
        && date.bytes().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == b'-' } else { c.is_ascii_digit() });
    ok.then_some(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory with a one-row database in it, removed on drop.
    struct Fixture {
        root: PathBuf,
        db: String,
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("lms-stats-backup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let db = root.join("live.db").to_string_lossy().into_owned();
            let conn = crate::db::open(&db).unwrap();
            conn.execute_batch(
                "INSERT INTO requests (ts, endpoint, model, prompt_tokens, completion_tokens, reasoning_tokens,
                                       total_tokens, stream, status, duration_ms)
                 VALUES (1, '/v1/chat/completions', 'm', 1, 2, 0, 3, 0, 200, 5)",
            )
            .unwrap();
            let dir = root.join("backups");
            Self { root, db, dir }
        }

        fn names(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn first_snapshot_is_a_readable_copy() {
        let f = Fixture::new("first");
        let path = snapshot_if_due(&f.db, &f.dir, Some("2026-10-06")).unwrap().unwrap();
        assert_eq!(path, f.dir.join("lms-stats-2026-10-06.db"));
        let copy = Connection::open(&path).unwrap();
        let n: i64 = copy.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(f.names(), vec!["lms-stats-2026-10-06.db"]);
    }

    #[test]
    fn next_snapshot_waits_a_week() {
        let f = Fixture::new("week");
        snapshot_if_due(&f.db, &f.dir, Some("2026-10-06")).unwrap().unwrap();
        assert!(snapshot_if_due(&f.db, &f.dir, Some("2026-10-06")).unwrap().is_none());
        assert!(snapshot_if_due(&f.db, &f.dir, Some("2026-10-12")).unwrap().is_none());
        assert!(snapshot_if_due(&f.db, &f.dir, Some("2026-10-13")).unwrap().is_some());
        assert_eq!(f.names(), vec!["lms-stats-2026-10-06.db", "lms-stats-2026-10-13.db"]);
    }

    #[test]
    fn keeps_newest_five_and_ignores_other_files() {
        let f = Fixture::new("prune");
        std::fs::create_dir_all(&f.dir).unwrap();
        std::fs::write(f.dir.join("notes.txt"), "keep me").unwrap();
        std::fs::write(f.dir.join("lms-stats-old.db"), "keep me").unwrap();
        for day in ["2026-08-01", "2026-08-08", "2026-08-15", "2026-08-22", "2026-08-29", "2026-09-05"] {
            snapshot_if_due(&f.db, &f.dir, Some(day)).unwrap().unwrap();
        }
        assert_eq!(
            f.names(),
            vec![
                "lms-stats-2026-08-08.db",
                "lms-stats-2026-08-15.db",
                "lms-stats-2026-08-22.db",
                "lms-stats-2026-08-29.db",
                "lms-stats-2026-09-05.db",
                "lms-stats-old.db",
                "notes.txt",
            ]
        );
    }

    #[test]
    fn snapshot_names_are_strict() {
        assert_eq!(snapshot_date("lms-stats-2026-10-06.db"), Some("2026-10-06"));
        assert_eq!(snapshot_date("lms-stats-2026-10-06.db.tmp"), None);
        assert_eq!(snapshot_date("lms-stats-2026-1-06.db"), None);
        assert_eq!(snapshot_date("lms-stats.db"), None);
    }
}
