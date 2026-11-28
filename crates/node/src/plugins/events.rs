//! Delivering lifecycle events to the plugins that subscribed
//! (docs/specs/plugin-system AC-17).
//!
//! Each subscribed plugin has its own bounded queue and one worker thread,
//! so events reach it in the order they happened and a plugin that never
//! returns stalls only its own queue: publishing never blocks, and a full
//! queue drops the newest event and counts it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};

use oss_ext::lifecycle::Event;

use super::registry::{Plugin, Registry, State};

/// The most events one plugin may have waiting.
pub const QUEUE: usize = 256;

struct Lane {
    id: String,
    kinds: Vec<String>,
    tx: SyncSender<Event>,
    delivered: Arc<AtomicU64>,
    dropped: AtomicU64,
}

/// What one plugin's lane has done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Events its worker has finished delivering, answered or not.
    pub delivered: u64,
    /// Events dropped because its queue was full.
    pub dropped: u64,
}

/// Every subscribed plugin's queue.
#[derive(Default)]
pub struct Dispatcher {
    lanes: Vec<Lane>,
}

fn work(
    plugin: &Arc<Plugin>,
    rx: &std::sync::mpsc::Receiver<Event>,
    done: &AtomicU64,
    now: fn() -> u64,
) {
    while let Ok(event) = rx.recv() {
        let bytes = event.encode();
        // The answer is empty by the point's ceiling, and a failure is the
        // guard's to count: the plugin's breaker sees it.
        let _ = plugin.call(&event.tenant, "lifecycle", &bytes, 0, now);
        done.fetch_add(1, Ordering::Relaxed);
    }
}

impl Dispatcher {
    /// Starts a worker for every ready plugin that subscribed to an event.
    pub fn start(registry: &Registry, now: fn() -> u64) -> Self {
        let mut lanes = Vec::new();
        for plugin in registry.plugins() {
            let kinds = match (plugin.state(), plugin.manifest()) {
                (State::Ready, Some(m)) if !m.events().is_empty() => m.events().to_vec(),
                _ => continue,
            };
            let (tx, rx) = sync_channel::<Event>(QUEUE);
            let delivered = Arc::new(AtomicU64::new(0));
            let (p, d) = (Arc::clone(plugin), Arc::clone(&delivered));
            let spawned = std::thread::Builder::new()
                .name(format!("plugin-events-{}", plugin.id()))
                .spawn(move || work(&p, &rx, &d, now));
            if spawned.is_err() {
                continue;
            }
            lanes.push(Lane {
                id: plugin.id().to_owned(),
                kinds,
                tx,
                delivered,
                dropped: AtomicU64::new(0),
            });
        }
        Self { lanes }
    }

    /// Offers `event` to every plugin subscribed to its kind. Never blocks.
    pub fn publish(&self, event: &Event) {
        for lane in self.lanes.iter().filter(|l| l.kinds.contains(&event.kind)) {
            match lane.tx.try_send(event.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    lane.dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    /// One plugin's counters, none when it has no lane.
    pub fn counts(&self, id: &str) -> Option<Counts> {
        self.lanes.iter().find(|l| l.id == id).map(|l| Counts {
            delivered: l.delivered.load(Ordering::Relaxed),
            dropped: l.dropped.load(Ordering::Relaxed),
        })
    }
}
