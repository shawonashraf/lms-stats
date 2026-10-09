# lms-stats

Rust proxy in front of LM Studio that counts tokens per request into SQLite
and serves a dashboard. README.md is the full reference; this file is what
is not obvious from the code.

## Build and test

    cargo build --release
    cargo test                    # unit tests plus tests/proxy.rs against a mock LM Studio
    cargo clippy --all-targets

## Layout

- `src/proxy.rs` forwards and records, `src/usage.rs` extracts usage from JSON
  and SSE streams, `src/db.rs` is the SQLite layer, `src/api.rs` the JSON
  routes, `src/live.rs` in-flight tracking and the SSE change channel,
  `src/admin.rs` model load/unload through LM Studio's native `/api/v1` REST API,
  `src/backup.rs` weekly snapshots, `src/service.rs` the Windows service entry.
- `static/dashboard.html` is embedded at build time; rebuild after editing it.
- `docs/superpowers/` holds specs and plans from earlier work.

## Running as a service

Three equivalents, one per platform. Keep their defaults and behaviour in
step when changing one (upstream, listen, DB path, backups, restart on crash):

| Platform | Files | Database |
|---|---|---|
| Linux systemd | `lms-stats.service` | `/var/lib/lms-stats/lms-stats.db` |
| Windows | `install-service.ps1`, `uninstall-service.ps1` | `C:\ProgramData\lms-stats\lms-stats.db` |
| macOS launchd | `install-service.sh`, `uninstall-service.sh` | `~/Library/Application Support/lms-stats/lms-stats.db` |

On this Mac the LaunchAgent `com.shawonashraf.lms-stats` is installed and
serves port 1235. After `cargo build --release`, run `./install-service.sh`
again to pick up the new binary. Log: `~/Library/Logs/lms-stats.log`.
Backups go to `~/Documents/lms-stats` weekly.

## Database

- Default `LMS_DB` for `cargo run` is `./lms-stats.db`, gitignored. The
  service installs use the paths above, so a dev run and the service do not
  share data.
- To restore a backup, stop the service and copy the snapshot over the
  `LMS_DB` file. Snapshots are complete databases from `VACUUM INTO`.
- Never store prompts, responses, headers or client addresses. One row per
  counted request only.
