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

LM Studio currently reports zero token usage for `/v1/embeddings`, so embeddings rows show 0 tokens; the row is still recorded.

## How streaming is counted

LM Studio only reports usage on a stream when `stream_options.include_usage` is
set. The proxy sets it, reads the final usage chunk, and drops that chunk again
unless the client asked for it itself.

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
