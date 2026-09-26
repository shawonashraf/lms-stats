# lms-stats

A small proxy in front of [LM Studio](https://lmstudio.ai) that counts tokens
per request and shows them on a dashboard. LM Studio does not expose
per-request usage anywhere; this does.

Single Rust binary, SQLite file, no auth. Meant for a trusted LAN.

## Run

    cargo run --release

Env vars (all optional):

| Var            | Default                     | Meaning                              |
|----------------|-----------------------------|--------------------------------------|
| `LMS_UPSTREAM` | `http://192.168.0.166:1234` | LM Studio base URL, plain `http://` (built without TLS) |
| `LMS_LISTEN`   | `0.0.0.0:1235`              | Proxy and dashboard bind address     |
| `LMS_DB`       | `./lms-stats.db`            | SQLite file, created if missing      |

Point your OpenAI-compatible clients at `http://<this-host>:1235/v1` instead of
LM Studio. Every path and method is forwarded unchanged, including auth
headers. Only these are counted:

- `POST /v1/chat/completions`
- `POST /v1/completions`
- `POST /v1/embeddings`

Dashboard: `http://<this-host>:1235/dashboard` (`/` redirects there).

## Dashboard

~[dashboard screenshot](./dashboard.png)

- Total tokens for the selected range, split into prompt, output and
  reasoning. Output is `completion_tokens - reasoning_tokens`, because LM
  Studio folds reasoning into `completion_tokens`.
- Bucketed chart by day, week (Monday start), month or year, with presets for
  today, 7 days, 30 days, this year, all time. Filter by model.
- Per-request table, newest first, with "Load more" pagination. Rows with a
  status of 400 or above are shown in red.
- Live: requests in flight show at the top of the table as a "generating"
  row with a running token estimate (one streamed chunk is roughly one token)
  and elapsed time, and the totals reload the moment a request finishes. The
  page listens on `/api/events`; a 15-second poll is the fallback.
- Chart.js and the IBM Plex Sans font load from CDNs; without internet the
  numbers and table still render, the chart does not.

Buckets use the proxy host's local timezone. Range presets use the browser's
timezone, so open the dashboard from a machine in the same timezone as the
proxy for the two to line up.

## JSON API

The dashboard is a static page over three endpoints you can use directly:

| Endpoint | Query params | Returns |
|---|---|---|
| `GET /api/requests` | `limit` (1–500, default 50), `offset`, `model` | Array of rows, newest first |
| `GET /api/aggregate` | `bucket` = `day` \| `week` \| `month` \| `year`, `from`, `to` (unix seconds), `model` | `{ totals, buckets }` |
| `GET /api/models` | | Array of model ids seen so far |
| `GET /api/active` | | `{ active: [...], completed }`: requests in flight (id, ts, endpoint, model, stream, chunks, elapsed_ms) and a count of rows written since start |
| `GET /api/events` | | Server-sent events; each event is the same snapshot, sent on every request start/finish and once a second |

Row fields: `id, ts, endpoint, model, prompt_tokens, completion_tokens,
reasoning_tokens, total_tokens, stream, status, duration_ms`.

## What is stored

One row per counted request: timestamp, endpoint, model, prompt / completion /
reasoning / total tokens, stream flag, status, duration. Never prompts,
responses, headers or client addresses.

The `status` column is the upstream HTTP status, except:

| Status | Meaning |
|---|---|
| `502` | LM Studio unreachable, or its stream failed mid-way. Counts are 0. |
| `499` | The client disconnected mid-stream. Counts are 0. |

LM Studio currently reports zero token usage for `/v1/embeddings`, so
embeddings rows show 0 tokens; the row is still recorded.

## How streaming is counted

LM Studio only reports usage on a stream when `stream_options.include_usage`
is set. The proxy sets it, reads the final usage chunk, and drops that chunk
again unless the client asked for it itself, so older SDKs that index
`choices[0]` are unaffected.

## When LM Studio is down

The proxy stays up and answers with `502` and an OpenAI-style JSON error:

    {"error":{"message":"LM Studio at http://192.168.0.166:1234 is unavailable: ...","type":"upstream_unavailable"}}

Counted requests appear in the dashboard with status 502; other paths are
forwarded but not recorded.

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

## GNOME Shell extension

`gnome-extension/lms-stats@shawonashraf/` adds a top-bar indicator: the
all-time token total next to a small monitor icon, and on click today / this
week / this month with request counts, plus an "Open dashboard" entry. It
polls the proxy at `http://127.0.0.1:1235` every 15 seconds (change the
`PROXY` constant at the top of `extension.js` if the proxy runs elsewhere).
GNOME Shell 48 to 50.

    ln -s "$PWD/gnome-extension/lms-stats@shawonashraf" ~/.local/share/gnome-shell/extensions/
    gnome-extensions enable lms-stats@shawonashraf   # or after the next login

On Wayland the Shell only picks up new extensions at login, so log out and
back in once. Errors, if any, show up in `journalctl --user -b /usr/bin/gnome-shell`.

## Development

    cargo test                    # unit tests plus an integration test against a mock LM Studio
    cargo clippy --all-targets

Layout: `src/proxy.rs` forwards and records, `src/usage.rs` extracts usage
from JSON and SSE streams, `src/db.rs` is the SQLite layer, `src/api.rs` the
JSON routes, `static/dashboard.html` the page (embedded at build time).
