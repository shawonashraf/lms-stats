# LM Studio token-counting proxy and dashboard

Date: 2026-09-26

## Goal

LM Studio (OpenAI-compatible, at `http://192.168.0.166:1234/v1`) does not
report per-request token usage anywhere visible. `lms-stats` is a single Rust
binary that sits in front of LM Studio, records token usage for every counted
request in SQLite, and serves a dashboard showing per-request rows and
aggregates bucketed by day, week, month or year.

Clients point at the proxy instead of LM Studio. Nothing else changes for them.

## Non-goals

- Storing prompts, responses, headers, API keys or client IPs. Only counts.
- Authentication on the proxy or dashboard. It runs on a trusted LAN.
- Multi-upstream routing, load balancing, retries, caching.
- Cost estimation.

## Configuration

Environment variables, all optional:

| Var            | Default                       | Meaning                    |
|----------------|-------------------------------|----------------------------|
| `LMS_UPSTREAM` | `http://192.168.0.166:1234`   | LM Studio base URL (no `/v1`) |
| `LMS_LISTEN`   | `0.0.0.0:1235`                | Proxy + dashboard bind address |
| `LMS_DB`       | `./lms-stats.db`              | SQLite file path (created if missing) |

## Components

### Proxy (`src/proxy.rs`)

Axum fallback handler. Every method and path is forwarded to
`LMS_UPSTREAM + path + query` with request headers copied (minus hop-by-hop
headers: `host`, `connection`, `transfer-encoding`, `content-length` is
recomputed). Response status and headers are copied back.

Counted endpoints (POST only):

- `/v1/chat/completions`
- `/v1/completions`
- `/v1/embeddings`

For a counted request:

1. Read the full request body. Parse as JSON. If it does not parse, forward
   as-is and do not count.
2. Record `model` (string, or `""`) and `stream` (bool, default false).
3. If `stream` is true and `stream_options.include_usage` is not already
   true, set it to true and remember `client_wanted_usage = false`.
4. Forward to upstream.
5. Non-streaming: buffer the response body, extract `usage`, forward body
   unchanged.
6. Streaming: forward bytes as they arrive. A tap splits the byte stream on
   `\n`, and for each `data: {...}` line parses JSON and looks for a top-level
   `usage` object. When `client_wanted_usage` is false, a chunk whose
   `choices` is empty and that carries `usage` is dropped instead of forwarded
   (older clients index `choices[0]`). Everything else is forwarded verbatim.
7. After the response finishes (either mode), insert one row. If no usage was
   found (error status, malformed upstream), insert the row with zero counts
   and the upstream status so failures are visible in the table.

Usage extraction: `prompt_tokens`, `completion_tokens`, `total_tokens`,
`completion_tokens_details.reasoning_tokens` (default 0). Embeddings responses
only have `prompt_tokens` and `total_tokens`; missing fields default to 0.

Errors: if LM Studio cannot be reached (connection refused, timeout, DNS),
the proxy returns `502` with an OpenAI-style JSON body so SDK clients surface
a readable message:

```json
{"error":{"message":"LM Studio at http://192.168.0.166:1234 is unavailable: <cause>","type":"upstream_unavailable"}}
```

For a counted request this also inserts a row with zero counts and status
`502`, so outages are visible in the dashboard table. A DB write failure is
logged to stderr and never affects the response.

### Storage (`src/db.rs`)

SQLite via `rusqlite` with the `bundled` feature. One `Connection` behind a
`std::sync::Mutex` (a single local user; contention is not a concern).

```sql
CREATE TABLE IF NOT EXISTS requests (
  id                INTEGER PRIMARY KEY,
  ts                INTEGER NOT NULL,   -- unix seconds, request start
  endpoint          TEXT    NOT NULL,   -- path, e.g. /v1/chat/completions
  model             TEXT    NOT NULL,
  prompt_tokens     INTEGER NOT NULL,
  completion_tokens INTEGER NOT NULL,
  reasoning_tokens  INTEGER NOT NULL,
  total_tokens      INTEGER NOT NULL,
  stream            INTEGER NOT NULL,   -- 0/1
  status            INTEGER NOT NULL,   -- upstream HTTP status
  duration_ms       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS requests_ts ON requests(ts);
```

Note: `completion_tokens` as reported by LM Studio already includes
`reasoning_tokens`. The dashboard shows `output = completion - reasoning`.

Queries:

- `insert(row)`
- `list(limit, offset, model: Option<&str>) -> Vec<Row>` newest first
- `aggregate(bucket, from, to, model) -> Vec<Bucket>` grouped by
  `strftime(fmt, ts, 'unixepoch', 'localtime')` where `fmt` is
  `%Y-%m-%d` (day), `%Y-W%W` (week), `%Y-%m` (month), `%Y` (year). Each bucket
  has request count and summed prompt / completion / reasoning / total tokens.
- `models() -> Vec<String>` distinct models, for the filter dropdown.

### Dashboard (`src/api.rs`, `static/dashboard.html`)

Routes:

- `GET /dashboard` – serves `dashboard.html` embedded with `include_str!`.
- `GET /api/requests?limit=50&offset=0&model=` – JSON array of rows.
- `GET /api/aggregate?bucket=day&from=<unix>&to=<unix>&model=` – JSON
  `{ totals: {...}, buckets: [{ key, requests, prompt, completion, reasoning, total }] }`.
- `GET /api/models` – JSON array of model ids.

Page (vanilla JS, Chart.js 4 from cdnjs):

- Controls: bucket selector (day / week / month / year), range presets
  (today, 7 days, 30 days, this year, all), model filter.
- Totals row for the selected range: requests, prompt, output, reasoning,
  total tokens.
- Stacked bar chart, one bar per bucket, series: prompt, output, reasoning.
- Per-request table, newest first, columns: time, model, endpoint, prompt,
  output, reasoning, total, stream, status, duration. Paginated with a
  "load more" button.

Range presets are computed in the browser and sent as unix seconds; bucketing
uses the server's local timezone, which is the same machine for this use.

## Deployment

`lms-stats.service` in the repo root is a system-level systemd unit: runs the
release binary as the login user, `Restart=always` with a 3 s delay, DB under
`/var/lib/lms-stats` via `StateDirectory`, env vars set in the unit. Install
is manual (`cargo build --release`, copy to `/etc/systemd/system/`,
`systemctl enable --now lms-stats`); the README documents it.

## Dependencies

`axum`, `tokio` (full), `reqwest` (stream, json), `rusqlite` (bundled),
`serde`, `serde_json`, `futures-util`, `bytes`. No others.

## Testing

- Unit: SSE tap given a fixed byte sequence (split at awkward boundaries)
  yields the usage object and drops the usage-only chunk when asked to.
- Unit: `aggregate` on an in-memory DB with a handful of rows spanning two
  days returns the expected buckets and totals for each bucket size.
- Integration: spawn a mock upstream (axum) that returns a canned streaming
  and a canned non-streaming response; run the proxy against it; assert the
  client receives the bytes unchanged (minus dropped usage chunk) and a row
  with the right counts lands in the DB.
- Manual: `cargo run`, point a curl at `:1235/v1/chat/completions` with and
  without `stream`, open `/dashboard`, see the rows.

## Success criteria

1. A streamed and a non-streamed chat request through the proxy each produce
   one row with the same counts LM Studio reports directly.
2. An embeddings request produces a row with `prompt_tokens` > 0.
3. A client that did not ask for usage never receives a chunk with empty
   `choices`.
4. `/dashboard` renders totals, chart and table for day/week/month/year.
5. With LM Studio stopped, a chat request through the proxy returns `502`
   with the JSON error above, and a row with status 502 appears in the table.
6. `systemd-analyze verify lms-stats.service` passes.
