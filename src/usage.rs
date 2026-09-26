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
    /// `data:` chunks with a non-empty `choices`, i.e. content or finish events; ~tokens generated.
    pub events: u64,
}

impl SseTap {
    pub fn new(drop_usage_chunk: bool) -> Self {
        Self { buf: Vec::new(), drop_usage_chunk, usage: None, events: 0 }
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
        let choices_empty = v.get("choices").and_then(Value::as_array).is_none_or(|a| a.is_empty());
        if !choices_empty {
            self.events += 1;
        }
        let Some(u) = from_json(&v) else { return true };
        self.usage = Some(u);
        !(self.drop_usage_chunk && choices_empty)
    }
}

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
    fn tap_counts_content_events() {
        let input = format!("{CHUNK1}{CHUNK2}{USAGE}{DONE}");
        let mut tap = SseTap::new(true);
        run(&mut tap, &input, 9);
        // Two chunks carry choices (content delta + finish); the usage-only chunk and [DONE] do not.
        assert_eq!(tap.events, 2);
    }

    #[test]
    fn tap_flushes_partial_trailing_line() {
        let mut tap = SseTap::new(true);
        assert_eq!(run(&mut tap, "data: {\"choices\":[]", 1024), "data: {\"choices\":[]");
        assert_eq!(tap.usage, None);
    }
}
