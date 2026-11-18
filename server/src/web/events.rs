//! Backend event fan-out for `GET /api/events` (SSE) with a short replay
//! log, so a browser that dropped its stream (phone backgrounded, network
//! handover, proxy timeout) gets the events it missed when it reconnects
//! instead of losing chat tokens or ingest progress.
//!
//! Every event gets an id `<boot>:<seq>`. `boot` changes on each server
//! start, so ids from a previous process are never confused with new ones.
//! A reconnecting client sends the last id it saw (the standard
//! `Last-Event-ID` header on EventSource's own retry, or `?since=` when the
//! page opens a fresh EventSource) and receives every buffered event after
//! it, followed by the live stream.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tauri::AppHandle;
use tokio::sync::broadcast;

/// Keep at most this many events …
const MAX_EVENTS: usize = 2000;
/// … and at most this many bytes of serialized payload.
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct Logged {
    pub seq: u64,
    pub data: Arc<str>,
}

pub struct EventLog {
    pub boot: String,
    inner: Mutex<LogInner>,
    tx: broadcast::Sender<Logged>,
}

struct LogInner {
    next: u64,
    bytes: usize,
    buf: VecDeque<Logged>,
}

impl EventLog {
    /// Start relaying the app's events through the log.
    pub fn spawn(app: &AppHandle) -> Arc<Self> {
        let boot = format!(
            "{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let (tx, _) = broadcast::channel(4096);
        let log = Arc::new(EventLog {
            boot,
            inner: Mutex::new(LogInner { next: 1, bytes: 0, buf: VecDeque::new() }),
            tx,
        });
        let mut rx = app.subscribe_events();
        let relay = log.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let data: Arc<str> = serde_json::json!({"event": ev.event, "payload": ev.payload})
                            .to_string()
                            .into();
                        relay.push(data);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("[web] event log lagged, dropped {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        log
    }

    fn push(&self, data: Arc<str>) {
        let item = {
            let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let item = Logged { seq: g.next, data };
            g.next += 1;
            g.bytes += item.data.len();
            g.buf.push_back(item.clone());
            while g.buf.len() > MAX_EVENTS || (g.bytes > MAX_BYTES && g.buf.len() > 1) {
                if let Some(old) = g.buf.pop_front() {
                    g.bytes -= old.data.len();
                }
            }
            item
        };
        let _ = self.tx.send(item);
    }

    /// Parse a client-supplied `<boot>:<seq>` id; ids from another boot
    /// (or garbage) mean "nothing to replay".
    pub fn parse_since(&self, raw: &str) -> Option<u64> {
        let (boot, seq) = raw.trim().split_once(':')?;
        if boot != self.boot {
            return None;
        }
        seq.parse().ok()
    }

    /// Subscribe to live events and collect the buffered ones after `since`.
    /// Subscribing first means nothing falls between the two; the caller
    /// skips live events already replayed.
    pub fn subscribe_from(&self, since: Option<u64>) -> (Vec<Logged>, broadcast::Receiver<Logged>) {
        let rx = self.tx.subscribe();
        let replay = match since {
            Some(s) => {
                let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                g.buf.iter().filter(|e| e.seq > s).cloned().collect()
            }
            None => Vec::new(),
        };
        (replay, rx)
    }

    pub fn id_of(&self, seq: u64) -> String {
        format!("{}:{}", self.boot, seq)
    }
}
