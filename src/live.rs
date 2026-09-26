//! In-flight request tracking and a change channel for the dashboard.
//!
//! LM Studio only reports usage when a stream ends, so during a long generation
//! the only live signal is the chunk count passing through the tap. Each SSE
//! content chunk is roughly one token, which is good enough for a progress row.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Serialize)]
pub struct Active {
    pub id: u64,
    pub ts: i64,
    pub endpoint: String,
    pub model: String,
    pub stream: bool,
    /// SSE chunks with a non-empty `choices` seen so far; ~tokens generated.
    pub chunks: u64,
    pub elapsed_ms: u64,
    #[serde(skip)]
    started: Instant,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub active: Vec<Active>,
    /// Rows inserted since startup; the page reloads when this changes.
    pub completed: u64,
}

pub struct Live {
    next_id: AtomicU64,
    completed: AtomicU64,
    // ponytail: one mutex, locked per streamed chunk; fine for one LAN's worth of traffic.
    active: Mutex<BTreeMap<u64, Active>>,
    tx: broadcast::Sender<()>,
}

impl Default for Live {
    fn default() -> Self {
        Self::new()
    }
}

impl Live {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            completed: AtomicU64::new(0),
            active: Mutex::new(BTreeMap::new()),
            tx: broadcast::channel(16).0,
        }
    }

    pub fn start(&self, ts: i64, endpoint: &str, model: &str, stream: bool) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = Active {
            id,
            ts,
            endpoint: endpoint.to_string(),
            model: model.to_string(),
            stream,
            chunks: 0,
            elapsed_ms: 0,
            started: Instant::now(),
        };
        if let Ok(mut m) = self.active.lock() {
            m.insert(id, entry);
        }
        self.notify();
        id
    }

    pub fn progress(&self, id: u64, chunks: u64) {
        if let Ok(mut m) = self.active.lock()
            && let Some(a) = m.get_mut(&id)
        {
            a.chunks = chunks;
        }
    }

    pub fn finish(&self, id: u64) {
        if let Ok(mut m) = self.active.lock() {
            m.remove(&id);
        }
        self.completed.fetch_add(1, Ordering::Relaxed);
        self.notify();
    }

    pub fn snapshot(&self) -> Snapshot {
        let active = match self.active.lock() {
            Ok(m) => m
                .values()
                .map(|a| Active { elapsed_ms: a.started.elapsed().as_millis() as u64, ..a.clone() })
                .collect(),
            Err(_) => Vec::new(),
        };
        Snapshot { active, completed: self.completed.load(Ordering::Relaxed) }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    fn notify(&self) {
        // No subscribers is not an error.
        let _ = self.tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_progress_finish_roundtrip() {
        let live = Live::new();
        let mut rx = live.subscribe();
        assert_eq!(live.snapshot().active.len(), 0);

        let id = live.start(1_790_000_000, "/v1/chat/completions", "m", true);
        assert!(rx.try_recv().is_ok(), "start notifies");
        live.progress(id, 7);
        let snap = live.snapshot();
        assert_eq!(snap.active.len(), 1);
        assert_eq!((snap.active[0].id, snap.active[0].chunks, snap.active[0].model.as_str()), (id, 7, "m"));
        assert_eq!(snap.completed, 0);

        live.finish(id);
        assert!(rx.try_recv().is_ok(), "finish notifies");
        let snap = live.snapshot();
        assert_eq!(snap.active.len(), 0);
        assert_eq!(snap.completed, 1);

        live.progress(id, 99); // unknown id is ignored
        assert_eq!(live.snapshot().active.len(), 0);
    }
}
