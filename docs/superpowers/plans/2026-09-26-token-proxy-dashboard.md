# LM Studio Token Proxy + Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single Rust binary that proxies OpenAI-compatible requests to LM Studio, records per-request token usage in SQLite, and serves a dashboard with per-request rows and day/week/month/year aggregates.

**Architecture:** axum serves two things on one port: a fallback route that forwards everything to LM Studio (tapping the response for the `usage` object) and a small JSON API plus one embedded HTML page for the dashboard. rusqlite (bundled) stores one row per counted request behind a mutex. The dashboard is vanilla JS with Chart.js from a CDN.

**Tech Stack:** Rust 2024 edition, axum 0.8, tokio 1, reqwest 0.13 (no TLS, `stream` feature), rusqlite 0.40 (`bundled`), serde/serde_json, futures-util, bytes. Chart.js 4.4.1.

**Spec:** `docs/superpowers/specs/2026-09-26-token-proxy-dashboard-design.md`

## Global Constraints

- Env vars and defaults: `LMS_UPSTREAM=http://192.168.0.166:1234`, `LMS_LISTEN=0.0.0.0:1235`, `LMS_DB=./lms-stats.db`.
- Counted endpoints, POST only: `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`. Everything else is forwarded untouched and not recorded.
- Never store prompts, responses, headers, API keys or client IPs.
- Dependencies limited to: axum, tokio, reqwest, rusqlite, serde, serde_json, futures-util, bytes (plus dev-only: none).
- Commit style: imperative subject line, body explains why. No AI co-author or attribution trailers.
- Run `cargo test` before every commit. All tests must pass.

## File structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | dependencies |
| `src/main.rs` | read env, open DB, bind, serve |
| `src/lib.rs` | `AppState`, `router()`, module declarations (lib so `tests/` can build the router) |
| `src/db.rs` | schema, `Row`, insert / list / aggregate / models |
| `src/usage.rs` | `Usage` extraction from JSON; `SseTap` for streaming responses |
| `src/proxy.rs` | fallback handler: forward, tap, record |
| `src/api.rs` | `/dashboard`, `/api/requests`, `/api/aggregate`, `/api/models` |
| `static/dashboard.html` | the UI, embedded via `include_str!` |
| `tests/proxy.rs` | integration test with a mock upstream |
| `README.md` | how to run and point clients at it |

---

### Task 1: Project skeleton, config, and a running server

**Files:**
- Modify: `Cargo.toml`
- Create: `src/lib.rs`
- Modify: `src/main.rs`
- Create: `src/db.rs` (stub: `open` only), `src/api.rs` (stub router), `src/proxy.rs` (stub handler), `src/usage.rs` (empty)

**Interfaces:**
- Produces: `pub struct AppState { pub upstream: String, pub client: reqwest::Client, pub db: std::sync::Mutex<rusqlite::Connection> }` and `pub fn router(state: Arc<AppState>) -> axum::Router` in `src/lib.rs`. `db::open(path) -> rusqlite::Result<Connection>`.

- [ ] **Step 1: Dependencies**

Replace `Cargo.toml` with:

```toml
[package]
name = "lms-stats"
version = "0.1.0"
edition = "2024"

[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.13", default-features = false, features = ["stream", "json"] }
rusqlite = { version = "0.40", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
futures-util = "0.3"
bytes = "1"
```

`default-features = false` on reqwest drops TLS (upstream is plain http) and `system-proxy` (so `HTTP_PROXY` env vars can't hijack LAN traffic).

- [ ] **Step 2: Library root**

Create `src/lib.rs`:

```rust
pub mod api;
pub mod db;
pub mod proxy;
pub mod usage;

use std::sync::{Arc, Mutex};

pub struct AppState {
    /// Base URL of LM Studio without trailing slash, e.g. `http://192.168.0.166:1234`.
    pub upstream: String,
    pub client: reqwest::Client,
    pub db: Mutex<rusqlite::Connection>,
}

pub fn router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .merge(api::router())
        .fallback(proxy::handler)
        .with_state(state)
}
```

- [ ] **Step 3: Stubs so it compiles**

`src/db.rs`:

```rust
use rusqlite::Connection;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS requests (
  id                INTEGER PRIMARY KEY,
  ts                INTEGER NOT NULL,
  endpoint          TEXT    NOT NULL,
  model             TEXT    NOT NULL,
  prompt_tokens     INTEGER NOT NULL,
  completion_tokens INTEGER NOT NULL,
  reasoning_tokens  INTEGER NOT NULL,
  total_tokens      INTEGER NOT NULL,
  stream            INTEGER NOT NULL,
  status            INTEGER NOT NULL,
  duration_ms       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS requests_ts ON requests(ts);
";

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}
```

`src/api.rs`:

```rust
use std::sync::Arc;

use axum::Router;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
}
```

`src/proxy.rs`:

```rust
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
```

`src/usage.rs`: create it empty (one line: `// token usage extraction`).

- [ ] **Step 4: main**

Replace `src/main.rs`:

```rust
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
    });

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| panic!("bind {listen}: {e}"));
    eprintln!("lms-stats: listening on {listen}, upstream {upstream}, db {db_path}");
    axum::serve(listener, router(state)).await.unwrap();
}
```

- [ ] **Step 5: Build and smoke-test**

Run: `cargo build`
Expected: compiles with no errors (warnings about unused imports in stubs are fine).

Run: `LMS_LISTEN=127.0.0.1:1235 LMS_DB=/tmp/lms-test.db cargo run &` then `curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:1235/anything`; then `kill %1`.
Expected: `501`.

- [ ] **Step 6: Add `lms-stats.db*` to `.gitignore` and commit**

Append to `.gitignore`:

```
/lms-stats.db*
```

```bash
git add Cargo.toml Cargo.lock .gitignore src/lib.rs src/main.rs src/db.rs src/api.rs src/proxy.rs src/usage.rs
git commit -m "Scaffold axum server with env config and SQLite schema

Establishes the single-binary layout (lib + thin main) so integration
tests can build the router, and creates the requests table on startup."
```

---

### Task 2: Storage: insert, list, aggregate, models

**Files:**
- Modify: `src/db.rs`

**Interfaces:**
- Consumes: `db::open_memory()` from Task 1.
- Produces:
  - `pub struct Row { id: i64, ts: i64, endpoint: String, model: String, prompt_tokens: i64, completion_tokens: i64, reasoning_tokens: i64, total_tokens: i64, stream: bool, status: u16, duration_ms: i64 }` (all fields `pub`, derives `Debug, Clone, PartialEq, Serialize`)
  - `pub struct Totals { requests, prompt, completion, reasoning, total: i64 }` (derives `Default`)
  - `pub struct Bucket { key: String, #[serde(flatten)] totals: Totals }`
  - `pub enum BucketSize { Day, Week, Month, Year }` with `BucketSize::parse(&str) -> Option<BucketSize>`
  - `pub fn insert(conn: &Connection, row: &Row) -> rusqlite::Result<()>` (ignores `row.id`)
  - `pub fn list(conn: &Connection, limit: i64, offset: i64, model: Option<&str>) -> rusqlite::Result<Vec<Row>>` newest first
  - `pub fn aggregate(conn: &Connection, bucket: BucketSize, from: i64, to: i64, model: Option<&str>) -> rusqlite::Result<(Totals, Vec<Bucket>)>` with `from <= ts < to`
  - `pub fn models(conn: &Connection) -> rusqlite::Result<Vec<String>>`

- [ ] **Step 1: Write the failing tests**

Append to `src/db.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-25 12:00 UTC and 2026-09-26 12:00 UTC. Noon UTC keeps the calendar
    // day stable under any real timezone offset (bucketing uses server localtime).
    const DAY1: i64 = 1_790_337_600;
    const DAY2: i64 = 1_790_424_000;

    fn row(ts: i64, model: &str, prompt: i64, completion: i64, reasoning: i64) -> Row {
        Row {
            id: 0,
            ts,
            endpoint: "/v1/chat/completions".into(),
            model: model.into(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            reasoning_tokens: reasoning,
            total_tokens: prompt + completion,
            stream: false,
            status: 200,
            duration_ms: 10,
        }
    }

    fn seeded() -> Connection {
        let c = open_memory().unwrap();
        insert(&c, &row(DAY1, "a", 10, 5, 2)).unwrap();
        insert(&c, &row(DAY1 + 60, "b", 20, 8, 0)).unwrap();
        insert(&c, &row(DAY2, "a", 30, 12, 4)).unwrap();
        c
    }

    #[test]
    fn list_is_newest_first_and_filters_by_model() {
        let c = seeded();
        let all = list(&c, 50, 0, None).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].ts, DAY2);
        assert_eq!(all[2].ts, DAY1);
        assert_eq!(all[0].status, 200);

        let only_a = list(&c, 50, 0, Some("a")).unwrap();
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|r| r.model == "a"));

        let page = list(&c, 1, 1, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].ts, DAY1 + 60);
    }

    #[test]
    fn aggregate_by_day_week_month_year() {
        let c = seeded();
        let (totals, days) = aggregate(&c, BucketSize::Day, 0, i64::MAX, None).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].totals, Totals { requests: 2, prompt: 30, completion: 13, reasoning: 2, total: 43 });
        assert_eq!(days[1].totals, Totals { requests: 1, prompt: 30, completion: 12, reasoning: 4, total: 42 });
        assert_eq!(totals, Totals { requests: 3, prompt: 60, completion: 25, reasoning: 6, total: 85 });

        // Fri 25 and Sat 26 Sep 2026 share an ISO-ish week (%W, Monday start).
        let (_, weeks) = aggregate(&c, BucketSize::Week, 0, i64::MAX, None).unwrap();
        assert_eq!(weeks.len(), 1);
        let (_, months) = aggregate(&c, BucketSize::Month, 0, i64::MAX, None).unwrap();
        assert_eq!(months.len(), 1);
        let (_, years) = aggregate(&c, BucketSize::Year, 0, i64::MAX, None).unwrap();
        assert_eq!(years.len(), 1);
        assert_eq!(years[0].key, "2026");
    }

    #[test]
    fn aggregate_respects_range_and_model() {
        let c = seeded();
        let (totals, days) = aggregate(&c, BucketSize::Day, DAY2, i64::MAX, None).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(totals.requests, 1);

        let (totals, _) = aggregate(&c, BucketSize::Day, 0, i64::MAX, Some("b")).unwrap();
        assert_eq!(totals, Totals { requests: 1, prompt: 20, completion: 8, reasoning: 0, total: 28 });
    }

    #[test]
    fn models_are_distinct_and_sorted() {
        let c = seeded();
        assert_eq!(models(&c).unwrap(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn bucket_size_parses() {
        assert!(matches!(BucketSize::parse("day"), Some(BucketSize::Day)));
        assert!(matches!(BucketSize::parse("year"), Some(BucketSize::Year)));
        assert!(BucketSize::parse("hour").is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test db::`
Expected: compile errors, `Row`, `insert`, etc. not found.

- [ ] **Step 3: Implement**

Replace the top of `src/db.rs` (everything above `#[cfg(test)]`) with:

```rust
use rusqlite::{Connection, params};
use serde::Serialize;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS requests (
  id                INTEGER PRIMARY KEY,
  ts                INTEGER NOT NULL,
  endpoint          TEXT    NOT NULL,
  model             TEXT    NOT NULL,
  prompt_tokens     INTEGER NOT NULL,
  completion_tokens INTEGER NOT NULL,
  reasoning_tokens  INTEGER NOT NULL,
  total_tokens      INTEGER NOT NULL,
  stream            INTEGER NOT NULL,
  status            INTEGER NOT NULL,
  duration_ms       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS requests_ts ON requests(ts);
";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub id: i64,
    /// Unix seconds at request start.
    pub ts: i64,
    pub endpoint: String,
    pub model: String,
    pub prompt_tokens: i64,
    /// As reported by LM Studio; already includes `reasoning_tokens`.
    pub completion_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub stream: bool,
    pub status: u16,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub requests: i64,
    pub prompt: i64,
    pub completion: i64,
    pub reasoning: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bucket {
    pub key: String,
    #[serde(flatten)]
    pub totals: Totals,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BucketSize {
    Day,
    Week,
    Month,
    Year,
}

impl BucketSize {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }

    fn strftime(self) -> &'static str {
        match self {
            Self::Day => "%Y-%m-%d",
            Self::Week => "%Y-W%W",
            Self::Month => "%Y-%m",
            Self::Year => "%Y",
        }
    }
}

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn insert(conn: &Connection, r: &Row) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO requests (ts, endpoint, model, prompt_tokens, completion_tokens, reasoning_tokens,
                               total_tokens, stream, status, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            r.ts,
            r.endpoint,
            r.model,
            r.prompt_tokens,
            r.completion_tokens,
            r.reasoning_tokens,
            r.total_tokens,
            r.stream as i64,
            r.status as i64,
            r.duration_ms
        ],
    )?;
    Ok(())
}

pub fn list(conn: &Connection, limit: i64, offset: i64, model: Option<&str>) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, endpoint, model, prompt_tokens, completion_tokens, reasoning_tokens,
                total_tokens, stream, status, duration_ms
         FROM requests
         WHERE (?1 IS NULL OR model = ?1)
         ORDER BY id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(params![model, limit, offset], |r| {
        Ok(Row {
            id: r.get(0)?,
            ts: r.get(1)?,
            endpoint: r.get(2)?,
            model: r.get(3)?,
            prompt_tokens: r.get(4)?,
            completion_tokens: r.get(5)?,
            reasoning_tokens: r.get(6)?,
            total_tokens: r.get(7)?,
            stream: r.get::<_, i64>(8)? != 0,
            status: r.get::<_, i64>(9)? as u16,
            duration_ms: r.get(10)?,
        })
    })?;
    rows.collect()
}

pub fn aggregate(
    conn: &Connection,
    bucket: BucketSize,
    from: i64,
    to: i64,
    model: Option<&str>,
) -> rusqlite::Result<(Totals, Vec<Bucket>)> {
    let sql = format!(
        "SELECT strftime('{}', ts, 'unixepoch', 'localtime') AS k,
                COUNT(*), SUM(prompt_tokens), SUM(completion_tokens), SUM(reasoning_tokens), SUM(total_tokens)
         FROM requests
         WHERE ts >= ?1 AND ts < ?2 AND (?3 IS NULL OR model = ?3)
         GROUP BY k ORDER BY k",
        bucket.strftime()
    );
    let mut stmt = conn.prepare(&sql)?;
    let buckets = stmt
        .query_map(params![from, to, model], |r| {
            Ok(Bucket {
                key: r.get(0)?,
                totals: Totals {
                    requests: r.get(1)?,
                    prompt: r.get(2)?,
                    completion: r.get(3)?,
                    reasoning: r.get(4)?,
                    total: r.get(5)?,
                },
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut totals = Totals::default();
    for b in &buckets {
        totals.requests += b.totals.requests;
        totals.prompt += b.totals.prompt;
        totals.completion += b.totals.completion;
        totals.reasoning += b.totals.reasoning;
        totals.total += b.totals.total;
    }
    Ok((totals, buckets))
}

pub fn models(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT model FROM requests ORDER BY model")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    rows.collect()
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test db::`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add src/db.rs
git commit -m "Add request storage with list and bucketed aggregate queries

Bucketing is done in SQLite via strftime on localtime so day/week/month/
year all share one query; totals are summed from the buckets."
```

---

### Task 3: Usage extraction and the SSE tap

**Files:**
- Modify: `src/usage.rs`

**Interfaces:**
- Produces:
  - `#[derive(Debug, Default, Clone, Copy, PartialEq)] pub struct Usage { pub prompt: i64, pub completion: i64, pub reasoning: i64, pub total: i64 }`
  - `pub fn from_json(v: &serde_json::Value) -> Option<Usage>` reads top-level `usage`
  - `pub struct SseTap` with `new(drop_usage_chunk: bool)`, `feed(&mut self, chunk: &[u8]) -> Vec<u8>` (returns bytes to forward), `finish(&mut self) -> Vec<u8>` (flushes trailing partial line), and `pub usage: Option<Usage>`.

- [ ] **Step 1: Write the failing tests**

Replace `src/usage.rs` with just the tests for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK1: &str = "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n";
    const CHUNK2: &str = "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n";
    const USAGE: &str = "data: {\"id\":\"x\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"total_tokens\":16,\"completion_tokens_details\":{\"reasoning_tokens\":3}}}\n\n";
    const DONE: &str = "data: [DONE]\n\n";

    #[test]
    fn from_json_reads_usage_and_reasoning() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"usage":{"prompt_tokens":12,"completion_tokens":4,"total_tokens":16,"completion_tokens_details":{"reasoning_tokens":3}}}"#,
        )
        .unwrap();
        assert_eq!(from_json(&v), Some(Usage { prompt: 12, completion: 4, reasoning: 3, total: 16 }));
    }

    #[test]
    fn from_json_defaults_missing_fields_to_zero() {
        let v: serde_json::Value = serde_json::from_str(r#"{"usage":{"prompt_tokens":7,"total_tokens":7}}"#).unwrap();
        assert_eq!(from_json(&v), Some(Usage { prompt: 7, completion: 0, reasoning: 0, total: 7 }));
        let none: serde_json::Value = serde_json::from_str(r#"{"choices":[]}"#).unwrap();
        assert_eq!(from_json(&none), None);
    }

    /// Feed the whole stream in pieces of `n` bytes and concatenate what the tap forwards.
    fn run(tap: &mut SseTap, input: &str, n: usize) -> String {
        let mut out = Vec::new();
        for piece in input.as_bytes().chunks(n) {
            out.extend(tap.feed(piece));
        }
        out.extend(tap.finish());
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn tap_forwards_everything_when_client_wanted_usage() {
        let input = format!("{CHUNK1}{CHUNK2}{USAGE}{DONE}");
        let mut tap = SseTap::new(false);
        assert_eq!(run(&mut tap, &input, 7), input);
        assert_eq!(tap.usage, Some(Usage { prompt: 12, completion: 4, reasoning: 3, total: 16 }));
    }

    #[test]
    fn tap_drops_usage_only_chunk_when_asked() {
        let input = format!("{CHUNK1}{CHUNK2}{USAGE}{DONE}");
        let mut tap = SseTap::new(true);
        let out = run(&mut tap, &input, 5);
        assert!(!out.contains("usage"), "usage chunk must be dropped: {out}");
        assert!(out.contains("finish_reason"));
        assert!(out.ends_with(DONE));
        assert_eq!(tap.usage, Some(Usage { prompt: 12, completion: 4, reasoning: 3, total: 16 }));
    }

    #[test]
    fn tap_keeps_usage_chunk_that_also_has_choices() {
        let chunk = "data: {\"choices\":[{\"index\":0,\"delta\":{}}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n";
        let mut tap = SseTap::new(true);
        assert_eq!(run(&mut tap, chunk, 1024), chunk);
        assert_eq!(tap.usage.map(|u| u.total), Some(2));
    }

    #[test]
    fn tap_flushes_partial_trailing_line() {
        let mut tap = SseTap::new(true);
        assert_eq!(run(&mut tap, "data: {\"choices\":[]", 1024), "data: {\"choices\":[]");
        assert_eq!(tap.usage, None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test usage::`
Expected: compile errors, `Usage`, `from_json`, `SseTap` not found.

- [ ] **Step 3: Implement**

Insert above the `#[cfg(test)]` block in `src/usage.rs`:

```rust
use serde_json::Value;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Usage {
    pub prompt: i64,
    pub completion: i64,
    pub reasoning: i64,
    pub total: i64,
}

/// Reads the OpenAI-style top-level `usage` object. Missing counters are 0.
pub fn from_json(v: &Value) -> Option<Usage> {
    let u = v.get("usage")?.as_object()?;
    let n = |k: &str| u.get(k).and_then(Value::as_i64).unwrap_or(0);
    Some(Usage {
        prompt: n("prompt_tokens"),
        completion: n("completion_tokens"),
        total: n("total_tokens"),
        reasoning: u
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
    })
}

/// Sits on an SSE byte stream: forwards bytes unchanged, captures `usage` from any
/// `data:` line, and optionally drops the usage-only chunk (empty `choices`) that we
/// asked upstream for but the client did not.
pub struct SseTap {
    buf: Vec<u8>,
    drop_usage_chunk: bool,
    pub usage: Option<Usage>,
}

impl SseTap {
    pub fn new(drop_usage_chunk: bool) -> Self {
        Self { buf: Vec::new(), drop_usage_chunk, usage: None }
    }

    /// Feed incoming bytes; returns the bytes that should be forwarded to the client.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(chunk.len());
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            if self.keep(&line) {
                out.extend_from_slice(&line);
            }
        }
        out
    }

    /// Flush whatever partial line remains at end of stream.
    pub fn finish(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.buf);
        if self.keep(&rest) { rest } else { Vec::new() }
    }

    fn keep(&mut self, line: &[u8]) -> bool {
        let Some(payload) = line.strip_prefix(b"data:") else { return true };
        // `[DONE]` and anything non-JSON fails to parse and is forwarded as-is.
        let Ok(v) = serde_json::from_slice::<Value>(payload.trim_ascii()) else { return true };
        let Some(u) = from_json(&v) else { return true };
        self.usage = Some(u);
        let choices_empty = v.get("choices").and_then(Value::as_array).is_none_or(|a| a.is_empty());
        !(self.drop_usage_chunk && choices_empty)
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test usage::`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add src/usage.rs
git commit -m "Add usage extraction and SSE tap for streamed responses

LM Studio only reports usage on streams when stream_options.include_usage
is set, so the proxy will inject it; the tap captures the usage chunk and
can drop it for clients that never asked (older SDKs index choices[0])."
```

---

### Task 4: The proxy handler

**Files:**
- Modify: `src/proxy.rs`
- Create: `tests/proxy.rs`

**Interfaces:**
- Consumes: `AppState`, `router` (Task 1); `db::{Row, insert, list, open_memory}` (Task 2); `usage::{Usage, from_json, SseTap}` (Task 3).
- Produces: `pub async fn handler(State<Arc<AppState>>, Request) -> Response`.

- [ ] **Step 1: Write the failing integration test**

Create `tests/proxy.rs`:

```rust
use std::sync::{Arc, Mutex};

use axum::{Router, body::Body, extract::Request, response::Response, routing::post};
use lms_stats::{AppState, db, router};

const STREAM_BODY: &str = "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
data: {\"id\":\"x\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":4,\"total_tokens\":16,\"completion_tokens_details\":{\"reasoning_tokens\":3}}}\n\n\
data: [DONE]\n\n";

const JSON_BODY: &str = r#"{"id":"y","choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#;

/// Mock LM Studio: records the last request body, answers streaming or JSON based on `stream`.
async fn mock_upstream(seen: Arc<Mutex<Vec<String>>>) -> String {
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(move |req: Request| {
                let seen = seen.clone();
                async move {
                    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX).await.unwrap();
                    let text = String::from_utf8(bytes.to_vec()).unwrap();
                    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                    seen.lock().unwrap().push(text);
                    if v["stream"].as_bool().unwrap_or(false) {
                        Response::builder()
                            .header("content-type", "text/event-stream")
                            .body(Body::from(STREAM_BODY))
                            .unwrap()
                    } else {
                        Response::builder()
                            .header("content-type", "application/json")
                            .body(Body::from(JSON_BODY))
                            .unwrap()
                    }
                }
            }),
        )
        .route("/v1/models", axum::routing::get(|| async { r#"{"data":[]}"# }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

async fn start(upstream: String) -> (String, Arc<AppState>) {
    let state = Arc::new(AppState {
        upstream,
        client: reqwest::Client::new(),
        db: Mutex::new(db::open_memory().unwrap()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}

async fn wait_for_rows(state: &AppState, n: usize) -> Vec<db::Row> {
    for _ in 0..50 {
        let rows = db::list(&state.db.lock().unwrap(), 50, 0, None).unwrap();
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected {n} rows");
}

#[tokio::test]
async fn non_streaming_request_is_forwarded_and_recorded() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen.clone()).await).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m1","messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), JSON_BODY);

    let rows = wait_for_rows(&state, 1).await;
    let r = &rows[0];
    assert_eq!((r.model.as_str(), r.endpoint.as_str(), r.stream, r.status), ("m1", "/v1/chat/completions", false, 200));
    assert_eq!((r.prompt_tokens, r.completion_tokens, r.reasoning_tokens, r.total_tokens), (5, 2, 0, 7));
}

#[tokio::test]
async fn streaming_request_injects_include_usage_and_strips_usage_chunk() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen.clone()).await).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m2","messages":[],"stream":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(!body.contains("usage"), "usage chunk leaked to client: {body}");
    assert!(body.contains("\"content\":\"hi\""));
    assert!(body.ends_with("data: [DONE]\n\n"));

    let sent: serde_json::Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
    assert_eq!(sent["stream_options"]["include_usage"], serde_json::Value::Bool(true));

    let rows = wait_for_rows(&state, 1).await;
    let r = &rows[0];
    assert!(r.stream);
    assert_eq!((r.prompt_tokens, r.completion_tokens, r.reasoning_tokens, r.total_tokens), (12, 4, 3, 16));
}

#[tokio::test]
async fn streaming_client_that_asked_for_usage_keeps_the_chunk() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen.clone()).await).await;
    let body = reqwest::Client::new()
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m3","messages":[],"stream":true,"stream_options":{"include_usage":true}}"#)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, STREAM_BODY);
    let rows = wait_for_rows(&state, 1).await;
    assert_eq!(rows[0].total_tokens, 16);
}

#[tokio::test]
async fn uncounted_paths_pass_through_without_rows() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (proxy, state) = start(mock_upstream(seen).await).await;
    let body = reqwest::get(format!("{proxy}/v1/models")).await.unwrap().text().await.unwrap();
    assert_eq!(body, r#"{"data":[]}"#);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(db::list(&state.db.lock().unwrap(), 50, 0, None).unwrap().is_empty());
}

#[tokio::test]
async fn unreachable_upstream_returns_502_and_records_failure() {
    let (proxy, state) = start("http://127.0.0.1:1".to_string()).await;
    let resp = reqwest::Client::new()
        .post(format!("{proxy}/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"m","messages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);
    let body: serde_json::Value = resp.json().await.unwrap();
    let msg = body["error"]["message"].as_str().unwrap();
    assert!(msg.contains("LM Studio at http://127.0.0.1:1 is unavailable"), "{msg}");
    assert_eq!(body["error"]["type"], "upstream_unavailable");

    let rows = wait_for_rows(&state, 1).await;
    assert_eq!((rows[0].status, rows[0].total_tokens, rows[0].model.as_str()), (502, 0, "m"));
}

#[tokio::test]
async fn unreachable_upstream_on_uncounted_path_records_nothing() {
    let (proxy, state) = start("http://127.0.0.1:1".to_string()).await;
    let resp = reqwest::get(format!("{proxy}/v1/models")).await.unwrap();
    assert_eq!(resp.status(), 502);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(db::list(&state.db.lock().unwrap(), 50, 0, None).unwrap().is_empty());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test proxy`
Expected: all 6 tests FAIL (the stub returns 501).

- [ ] **Step 3: Implement the handler**

Replace `src/proxy.rs`:

```rust
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::Value;

use crate::{
    AppState, db,
    usage::{self, SseTap, Usage},
};

const COUNTED: &[&str] = &["/v1/chat/completions", "/v1/completions", "/v1/embeddings"];

/// Headers that must not be copied between the two hops. `content-length` is
/// recomputed; `accept-encoding` is dropped so upstream never compresses a body
/// we need to read.
const SKIP_HEADERS: &[&str] = &[
    "host",
    "connection",
    "transfer-encoding",
    "content-length",
    "keep-alive",
    "upgrade",
    "proxy-connection",
    "accept-encoding",
];

/// What we learned from a counted request body.
struct Counted {
    model: String,
    stream: bool,
    /// The client itself asked for `stream_options.include_usage`, so the usage
    /// chunk is forwarded rather than dropped.
    client_wanted_usage: bool,
}

pub async fn handler(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let started = Instant::now();
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);

    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    let path_and_query = parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| path.clone());
    let mut body = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b.to_vec(),
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let mut counted = None;
    if parts.method == Method::POST && COUNTED.contains(&path.as_str()) {
        if let Ok(mut v) = serde_json::from_slice::<Value>(&body) && v.is_object() {
            let model = v.get("model").and_then(Value::as_str).unwrap_or("").to_string();
            let stream = v.get("stream").and_then(Value::as_bool).unwrap_or(false);
            let mut client_wanted_usage = true;
            if stream {
                client_wanted_usage = v.pointer("/stream_options/include_usage").and_then(Value::as_bool).unwrap_or(false);
                if !client_wanted_usage {
                    if !v["stream_options"].is_object() {
                        v["stream_options"] = serde_json::json!({});
                    }
                    v["stream_options"]["include_usage"] = Value::Bool(true);
                    body = serde_json::to_vec(&v).expect("re-serialize json");
                }
            }
            counted = Some(Counted { model, stream, client_wanted_usage });
        }
    }

    let mut upstream = state.client.request(parts.method.clone(), format!("{}{}", state.upstream, path_and_query));
    for (k, v) in &parts.headers {
        if !SKIP_HEADERS.contains(&k.as_str()) {
            upstream = upstream.header(k, v);
        }
    }
    let resp = match upstream.body(body).send().await {
        Ok(r) => r,
        Err(e) => {
            // Make outages visible on the dashboard: one zero-count row with status 502.
            if let Some(c) = &counted {
                record(&state, ts, &path, c, StatusCode::BAD_GATEWAY, started, None);
            }
            return unavailable(&state.upstream, &e);
        }
    };
    let status = resp.status();
    let mut headers = HeaderMap::new();
    for (k, v) in resp.headers() {
        if !SKIP_HEADERS.contains(&k.as_str()) {
            headers.insert(k.clone(), v.clone());
        }
    }

    let Some(c) = counted else {
        return build(status, headers, Body::from_stream(resp.bytes_stream()));
    };

    if !c.stream {
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("upstream body error: {e}")).into_response(),
        };
        let found = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| usage::from_json(&v));
        record(&state, ts, &path, &c, status, started, found);
        return build(status, headers, Body::from(bytes));
    }

    // Streaming: forward chunks through the tap as they arrive; record once the
    // upstream stream ends (or the client goes away).
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(16);
    let state2 = state.clone();
    tokio::spawn(async move {
        let mut tap = SseTap::new(!c.client_wanted_usage);
        let mut upstream_body = resp.bytes_stream();
        while let Some(item) = upstream_body.next().await {
            match item {
                Ok(chunk) => {
                    let out = tap.feed(&chunk);
                    if !out.is_empty() && tx.send(Ok(out.into())).await.is_err() {
                        break; // client disconnected; dropping `upstream_body` aborts upstream
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(std::io::Error::other(e))).await;
                    break;
                }
            }
        }
        let rest = tap.finish();
        if !rest.is_empty() {
            let _ = tx.send(Ok(rest.into())).await;
        }
        record(&state2, ts, &path, &c, status, started, tap.usage);
    });
    let client_body = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|item| (item, rx)) });
    build(status, headers, Body::from_stream(client_body))
}

/// OpenAI-style error body so SDK clients surface a readable message instead of a bare 502.
fn unavailable(upstream: &str, err: &reqwest::Error) -> Response {
    let body = serde_json::json!({
        "error": {
            "message": format!("LM Studio at {upstream} is unavailable: {err}"),
            "type": "upstream_unavailable"
        }
    });
    (StatusCode::BAD_GATEWAY, axum::Json(body)).into_response()
}

fn build(status: StatusCode, headers: HeaderMap, body: Body) -> Response {
    let mut r = Response::new(body);
    *r.status_mut() = status;
    *r.headers_mut() = headers;
    r
}

fn record(state: &AppState, ts: i64, endpoint: &str, c: &Counted, status: StatusCode, started: Instant, found: Option<Usage>) {
    let u = found.unwrap_or_default();
    let row = db::Row {
        id: 0,
        ts,
        endpoint: endpoint.to_string(),
        model: c.model.clone(),
        prompt_tokens: u.prompt,
        completion_tokens: u.completion,
        reasoning_tokens: u.reasoning,
        total_tokens: u.total,
        stream: c.stream,
        status: status.as_u16(),
        duration_ms: started.elapsed().as_millis() as i64,
    };
    // ponytail: std Mutex held for one INSERT; move to a writer task if the lock ever shows up in profiles.
    let result = match state.db.lock() {
        Ok(conn) => db::insert(&conn, &row).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = result {
        eprintln!("lms-stats: db insert failed: {e}");
    }
}
```

Notes for the implementer:
- `if let ... && v.is_object()` is a let-chain, stable in edition 2024.
- axum and reqwest 0.13 both use the `http` 1.x crate, so `Method`, `StatusCode`, `HeaderMap` and header values are the same types across the two hops. If the compiler complains about mismatched `http` versions, run `cargo tree -i http` and align.
- `Body::from_stream` accepts any `Stream<Item = Result<T, E>>` where `T: Into<Bytes>` and `E: Into<BoxError>`; `std::io::Error` qualifies.

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: db 5 passed, usage 6 passed, proxy 6 passed.

- [ ] **Step 5: Commit**

```bash
git add src/proxy.rs tests/proxy.rs
git commit -m "Proxy requests to LM Studio and record token usage per request

Counted POSTs are parsed once to learn model and stream mode; streamed
requests get include_usage injected and the usage-only chunk is stripped
again unless the client asked for it. Rows are written after the response
finishes so a failed request still shows up with zero counts, and an
unreachable LM Studio answers with an OpenAI-style JSON error plus a
status-502 row."
```

---

### Task 5: Dashboard JSON API

**Files:**
- Modify: `src/api.rs`
- Modify: `tests/proxy.rs` (append API tests; they reuse `start`)

**Interfaces:**
- Consumes: `db::{list, aggregate, models, BucketSize, Row, Totals, Bucket}` (Task 2), `AppState`.
- Produces routes:
  - `GET /api/requests?limit=50&offset=0&model=` → `Vec<db::Row>` JSON
  - `GET /api/aggregate?bucket=day&from=0&to=<i64>&model=` → `{ "totals": Totals, "buckets": [Bucket] }`
  - `GET /api/models` → `["model-id", ...]`
  - `GET /dashboard` → HTML (Task 6 supplies the file; this task ships a placeholder)

- [ ] **Step 1: Write the failing tests**

Append to `tests/proxy.rs`:

```rust
#[tokio::test]
async fn api_lists_and_aggregates() {
    let (proxy, state) = start("http://127.0.0.1:1".to_string()).await;
    {
        let conn = state.db.lock().unwrap();
        for (ts, model, p, c) in [(1_790_337_600, "a", 10, 5), (1_790_424_000, "b", 20, 8)] {
            db::insert(
                &conn,
                &db::Row {
                    id: 0,
                    ts,
                    endpoint: "/v1/chat/completions".into(),
                    model: model.into(),
                    prompt_tokens: p,
                    completion_tokens: c,
                    reasoning_tokens: 0,
                    total_tokens: p + c,
                    stream: false,
                    status: 200,
                    duration_ms: 1,
                },
            )
            .unwrap();
        }
    }

    let rows: Vec<serde_json::Value> =
        reqwest::get(format!("{proxy}/api/requests?limit=1")).await.unwrap().json().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["model"], "b");

    let agg: serde_json::Value =
        reqwest::get(format!("{proxy}/api/aggregate?bucket=day")).await.unwrap().json().await.unwrap();
    assert_eq!(agg["totals"]["requests"], 2);
    assert_eq!(agg["totals"]["total"], 43);
    assert_eq!(agg["buckets"].as_array().unwrap().len(), 2);

    let agg_a: serde_json::Value =
        reqwest::get(format!("{proxy}/api/aggregate?bucket=month&model=a")).await.unwrap().json().await.unwrap();
    assert_eq!(agg_a["totals"]["requests"], 1);

    let bad = reqwest::get(format!("{proxy}/api/aggregate?bucket=hour")).await.unwrap();
    assert_eq!(bad.status(), 400);

    let models: Vec<String> = reqwest::get(format!("{proxy}/api/models")).await.unwrap().json().await.unwrap();
    assert_eq!(models, vec!["a", "b"]);

    let html = reqwest::get(format!("{proxy}/dashboard")).await.unwrap();
    assert_eq!(html.status(), 200);
    assert!(html.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test proxy api_lists_and_aggregates`
Expected: FAIL (the requests fall through to the proxy and get 502).

- [ ] **Step 3: Implement**

Create a placeholder `static/dashboard.html` containing exactly:

```html
<!doctype html><title>lms-stats</title><p>dashboard placeholder</p>
```

Replace `src/api.rs`:

```rust
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
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: everything passes, including `api_lists_and_aggregates`.

- [ ] **Step 5: Commit**

```bash
git add src/api.rs static/dashboard.html tests/proxy.rs
git commit -m "Add dashboard JSON API for requests, aggregates and models

Thin handlers over the db module; bucket size is validated to a 400 so
the UI gets a clear error instead of a SQL failure."
```

---

### Task 6: Dashboard page

**Files:**
- Modify: `static/dashboard.html` (replace placeholder)

**Interfaces:**
- Consumes: the three JSON routes from Task 5, exactly as specified there.

Design notes (from the dataviz skill's reference palette; series colors are its first three categorical slots, which validate all-pairs for color-vision deficiency in both light and dark mode):
- prompt = blue `#2a78d6` (dark `#3987e5`), output = orange `#eb6834` (dark `#d95926`), reasoning = aqua `#1baf7a` (dark `#199e70`).
- One y-axis. Stacked bars, thin, rounded ends, legend always shown (three series). Tooltip on hover. Text in ink tokens, never the series color.
- Output tokens shown = `completion - reasoning` (LM Studio's `completion_tokens` includes reasoning).

- [ ] **Step 1: Write the page**

Replace `static/dashboard.html` with:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>lms-stats</title>
<style>
  :root {
    color-scheme: light dark;
    --page: #f9f9f7; --surface: #fcfcfb; --ink: #0b0b0b; --ink-2: #52514e; --muted: #898781;
    --grid: #e1e0d9; --border: #c3c2b7;
    --prompt: #2a78d6; --output: #eb6834; --reasoning: #1baf7a;
  }
  @media (prefers-color-scheme: dark) {
    :root {
      --page: #0d0d0d; --surface: #1a1a19; --ink: #ffffff; --ink-2: #c3c2b7; --muted: #898781;
      --grid: #2c2c2a; --border: #383835;
      --prompt: #3987e5; --output: #d95926; --reasoning: #199e70;
    }
  }
  * { box-sizing: border-box; }
  body { margin: 0; padding: 24px 16px; background: var(--page); color: var(--ink);
         font: 14px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; }
  main { max-width: 1100px; margin: 0 auto; display: grid; gap: 16px; }
  h1 { font-size: 18px; margin: 0; font-weight: 600; }
  h1 small { color: var(--muted); font-weight: 400; margin-left: 8px; }
  .controls { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
  .controls label { color: var(--ink-2); display: flex; gap: 6px; align-items: center; }
  select, button { font: inherit; color: var(--ink); background: var(--surface); border: 1px solid var(--border);
                   border-radius: 6px; padding: 4px 8px; }
  button.active { border-color: var(--prompt); box-shadow: inset 0 0 0 1px var(--prompt); }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 8px; }
  .tile { background: var(--surface); border: 1px solid var(--grid); border-radius: 8px; padding: 10px 12px; }
  .tile .k { color: var(--muted); font-size: 12px; }
  .tile .v { font-size: 22px; font-variant-numeric: tabular-nums; font-weight: 600; }
  .card { background: var(--surface); border: 1px solid var(--grid); border-radius: 8px; padding: 12px; }
  .chart { position: relative; height: 280px; }
  .tablewrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-variant-numeric: tabular-nums; }
  th, td { text-align: right; padding: 6px 8px; border-bottom: 1px solid var(--grid); white-space: nowrap; }
  th { color: var(--muted); font-weight: 500; font-size: 12px; }
  th:nth-child(-n+3), td:nth-child(-n+3) { text-align: left; }
  td.err { color: #d03b3b; }
  .foot { display: flex; justify-content: space-between; align-items: center; color: var(--muted); margin-top: 8px; }
</style>
</head>
<body>
<main>
  <h1>lms-stats <small>token usage via proxy</small></h1>

  <div class="controls">
    <label>Bucket
      <select id="bucket">
        <option value="day">day</option><option value="week">week</option>
        <option value="month">month</option><option value="year">year</option>
      </select>
    </label>
    <label>Range</label>
    <span id="ranges">
      <button data-days="1">today</button>
      <button data-days="7" class="active">7 days</button>
      <button data-days="30">30 days</button>
      <button data-days="ytd">this year</button>
      <button data-days="all">all</button>
    </span>
    <label>Model <select id="model"><option value="">all</option></select></label>
  </div>

  <div class="tiles" id="tiles"></div>

  <div class="card">
    <div class="chart"><canvas id="chart"></canvas></div>
  </div>

  <div class="card">
    <div class="tablewrap">
      <table>
        <thead><tr>
          <th>time</th><th>model</th><th>endpoint</th>
          <th>prompt</th><th>output</th><th>reasoning</th><th>total</th>
          <th>stream</th><th>status</th><th>ms</th>
        </tr></thead>
        <tbody id="rows"></tbody>
      </table>
    </div>
    <div class="foot"><span id="count"></span><button id="more">load more</button></div>
  </div>
</main>

<script src="https://cdnjs.cloudflare.com/ajax/libs/Chart.js/4.4.1/chart.umd.min.js"></script>
<script>
const $ = (s) => document.querySelector(s);
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const fmt = (n) => Number(n).toLocaleString();
const PAGE = 50;

let range = "7";
let offset = 0;
let chart;

function rangeBounds() {
  const now = new Date();
  const to = Math.floor(now.getTime() / 1000) + 1;
  if (range === "all") return { from: 0, to };
  if (range === "ytd") return { from: Math.floor(new Date(now.getFullYear(), 0, 1).getTime() / 1000), to };
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  start.setDate(start.getDate() - (Number(range) - 1));
  return { from: Math.floor(start.getTime() / 1000), to };
}

function qs(extra) {
  const p = new URLSearchParams({ model: $("#model").value, ...extra });
  return p.toString();
}

async function loadModels() {
  const models = await (await fetch("/api/models")).json();
  const sel = $("#model");
  const current = sel.value;
  sel.innerHTML = '<option value="">all</option>' + models.map((m) => `<option>${m}</option>`).join("");
  sel.value = models.includes(current) ? current : "";
}

async function loadAggregate() {
  const { from, to } = rangeBounds();
  const data = await (await fetch(`/api/aggregate?${qs({ bucket: $("#bucket").value, from, to })}`)).json();
  const t = data.totals;
  const tiles = [
    ["requests", t.requests], ["prompt", t.prompt], ["output", t.completion - t.reasoning],
    ["reasoning", t.reasoning], ["total tokens", t.total],
  ];
  $("#tiles").innerHTML = tiles.map(([k, v]) => `<div class="tile"><div class="k">${k}</div><div class="v">${fmt(v)}</div></div>`).join("");

  const labels = data.buckets.map((b) => b.key);
  const series = [
    { label: "prompt", data: data.buckets.map((b) => b.prompt), backgroundColor: css("--prompt") },
    { label: "output", data: data.buckets.map((b) => b.completion - b.reasoning), backgroundColor: css("--output") },
    { label: "reasoning", data: data.buckets.map((b) => b.reasoning), backgroundColor: css("--reasoning") },
  ];
  if (chart) chart.destroy();
  chart = new Chart($("#chart"), {
    type: "bar",
    data: { labels, datasets: series.map((s) => ({ ...s, borderWidth: 1, borderColor: css("--surface"), borderRadius: 4, borderSkipped: false, maxBarThickness: 40 })) },
    options: {
      responsive: true, maintainAspectRatio: false,
      interaction: { mode: "index", intersect: false },
      plugins: {
        legend: { position: "top", labels: { color: css("--ink-2"), boxWidth: 12 } },
        tooltip: { callbacks: { label: (c) => ` ${c.dataset.label}: ${fmt(c.parsed.y)}` } },
      },
      scales: {
        x: { stacked: true, grid: { display: false }, ticks: { color: css("--muted") } },
        y: { stacked: true, beginAtZero: true, grid: { color: css("--grid") }, ticks: { color: css("--muted"), callback: fmt } },
      },
    },
  });
}

async function loadRows(reset) {
  if (reset) { offset = 0; $("#rows").innerHTML = ""; }
  const rows = await (await fetch(`/api/requests?${qs({ limit: PAGE, offset })}`)).json();
  offset += rows.length;
  $("#rows").insertAdjacentHTML("beforeend", rows.map((r) => `<tr>
    <td>${new Date(r.ts * 1000).toLocaleString()}</td>
    <td>${r.model || "—"}</td><td>${r.endpoint}</td>
    <td>${fmt(r.prompt_tokens)}</td><td>${fmt(r.completion_tokens - r.reasoning_tokens)}</td>
    <td>${fmt(r.reasoning_tokens)}</td><td>${fmt(r.total_tokens)}</td>
    <td>${r.stream ? "yes" : "no"}</td>
    <td class="${r.status >= 400 ? "err" : ""}">${r.status}</td><td>${fmt(r.duration_ms)}</td>
  </tr>`).join(""));
  $("#count").textContent = `${offset} shown`;
  $("#more").hidden = rows.length < PAGE;
}

async function refresh() {
  await loadModels();
  await Promise.all([loadAggregate(), loadRows(true)]);
}

$("#bucket").addEventListener("change", loadAggregate);
$("#model").addEventListener("change", refresh);
$("#more").addEventListener("click", () => loadRows(false));
$("#ranges").addEventListener("click", (e) => {
  const b = e.target.closest("button"); if (!b) return;
  range = b.dataset.days;
  document.querySelectorAll("#ranges button").forEach((x) => x.classList.toggle("active", x === b));
  loadAggregate();
});
window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", loadAggregate);

refresh();
setInterval(refresh, 15000);
</script>
</body>
</html>
```

- [ ] **Step 2: Run tests (they only check the route serves HTML)**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 3: Render and look at it**

Run: `LMS_LISTEN=127.0.0.1:1235 LMS_DB=/tmp/lms-dash.db cargo run &`, then seed a few rows through the proxy against the real LM Studio:

```bash
curl -s http://127.0.0.1:1235/v1/chat/completions -H 'Content-Type: application/json' \
  -d '{"model":"qwen/qwen3.6-35b-a3b","messages":[{"role":"user","content":"Say hi"}],"max_tokens":20}' >/dev/null
curl -s -N http://127.0.0.1:1235/v1/chat/completions -H 'Content-Type: application/json' \
  -d '{"model":"qwen/qwen3.6-35b-a3b","messages":[{"role":"user","content":"Say hi"}],"max_tokens":20,"stream":true}' >/dev/null
curl -s http://127.0.0.1:1235/v1/embeddings -H 'Content-Type: application/json' \
  -d '{"model":"text-embedding-nomic-embed-text-v1.5","input":"hello world"}' >/dev/null
curl -s 'http://127.0.0.1:1235/api/requests' | python3 -m json.tool
```

Expected: three rows; the chat rows have `total_tokens` > 0 and the streamed one has `stream: true`; the embeddings row has `prompt_tokens` > 0 and `completion_tokens` 0.

Open `http://127.0.0.1:1235/dashboard` in a browser (or use the claude-in-chrome skill to screenshot it). Check: tiles show the totals, the chart shows one stacked bar for today, table lists the three rows, switching bucket to month/year still shows one bar, model filter lists the two models. Then `kill %1`.

- [ ] **Step 4: Commit**

```bash
git add static/dashboard.html
git commit -m "Add dashboard page with totals, bucketed chart and request table

Vanilla JS over the JSON API; Chart.js from cdnjs. Output tokens are
shown as completion minus reasoning since LM Studio folds reasoning
into completion_tokens."
```

---

### Task 7: README and final verification

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write README**

```markdown
# lms-stats

A tiny proxy in front of LM Studio that counts tokens per request and shows them
on a dashboard. LM Studio does not expose per-request usage; this does.

## Run

    cargo run --release

Env vars (all optional):

| Var            | Default                     |
|----------------|-----------------------------|
| `LMS_UPSTREAM` | `http://192.168.0.166:1234` |
| `LMS_LISTEN`   | `0.0.0.0:1235`              |
| `LMS_DB`       | `./lms-stats.db`            |

Point your OpenAI-compatible clients at `http://<this-host>:1235/v1` instead of
LM Studio. Everything is forwarded; `/v1/chat/completions`, `/v1/completions`
and `/v1/embeddings` are counted.

Dashboard: `http://<this-host>:1235/dashboard`

## What is stored

One row per counted request: timestamp, endpoint, model, prompt / completion /
reasoning / total tokens, stream flag, upstream status, duration. Never prompts,
responses, headers or client addresses.

## How streaming is counted

LM Studio only reports usage on a stream when `stream_options.include_usage` is
set. The proxy sets it, reads the final usage chunk, and drops that chunk again
unless the client asked for it itself.
```

- [ ] **Step 2: Full verification against the spec's success criteria**

Run `cargo test` (all pass), then with the server running as in Task 6 step 3:

1. Non-streamed chat via proxy vs. direct to LM Studio with the same body: compare `usage` in the direct response to the newest row from `/api/requests`. Counts must match.
2. Streamed chat via proxy: row has `stream: true` and non-zero counts.
3. Embeddings: row has `prompt_tokens > 0`.
4. `curl -s -N ... "stream":true` through the proxy: output contains no `"usage"` line.
5. `/dashboard` renders with day/week/month/year switching.
6. With `LMS_UPSTREAM=http://127.0.0.1:9` (nothing listening), a chat request returns 502 with the JSON error naming LM Studio, and a status-502 row appears in `/api/requests`.

Record the actual numbers observed in the commit message body.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "Add README with run instructions and verification notes"
```

---

### Task 8: systemd service

**Files:**
- Create: `lms-stats.service`
- Modify: `README.md` (append a "Run as a service" section)

**Interfaces:**
- Consumes: the binary at `target/release/lms-stats` and the env vars from Global Constraints.

- [ ] **Step 1: Write the unit**

Create `lms-stats.service` in the repo root:

```ini
[Unit]
Description=lms-stats: token-counting proxy for LM Studio
After=network-online.target
Wants=network-online.target

[Service]
User=shawon
WorkingDirectory=/home/shawon/Projects/lms-stats
ExecStart=/home/shawon/Projects/lms-stats/target/release/lms-stats
Environment=LMS_UPSTREAM=http://192.168.0.166:1234
Environment=LMS_LISTEN=0.0.0.0:1235
Environment=LMS_DB=/var/lib/lms-stats/lms-stats.db
StateDirectory=lms-stats
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

`StateDirectory=lms-stats` makes systemd create `/var/lib/lms-stats` owned by `User`, so the DB lives outside the repo and survives `cargo clean`. `Restart=always` covers crashes; an unreachable LM Studio is not a crash (the proxy answers 502 and keeps running).

- [ ] **Step 2: Verify the unit parses**

Run: `systemd-analyze verify ./lms-stats.service`
Expected: no output (or only a warning that the ExecStart binary does not exist yet if `cargo build --release` has not been run; run `cargo build --release` first so the check is clean).

- [ ] **Step 3: Document installation**

Append to `README.md`:

```markdown
## Run as a service

    cargo build --release
    sudo cp lms-stats.service /etc/systemd/system/
    sudo systemctl daemon-reload
    sudo systemctl enable --now lms-stats
    systemctl status lms-stats
    journalctl -u lms-stats -f

The unit runs the release binary from this checkout as user `shawon`, keeps
the database in `/var/lib/lms-stats/`, and restarts on crash. After
`cargo build --release` again, `sudo systemctl restart lms-stats` picks up
the new binary. Edit the `Environment=` lines in the unit to change upstream,
port or DB path.

If LM Studio is down the proxy stays up and answers every request with
`502` and a JSON error naming the upstream; those requests appear in the
dashboard with status 502.
```

Do NOT install the unit yourself (no `sudo`); installation is the user's step.

- [ ] **Step 4: Commit**

```bash
git add lms-stats.service README.md
git commit -m "Add systemd unit for running the proxy as a service

Runs the release binary with Restart=always and keeps the DB under
/var/lib/lms-stats so it survives cargo clean. Install is documented,
not automated."
```
