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
