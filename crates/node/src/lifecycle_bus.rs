//! Where a node's index and node events go: a handle the services that
//! create and delete indexes hold from the start, and the plugin system
//! fills in once it is loaded (docs/specs/plugin-system AC-17).
//!
//! Until something is set, an event is dropped; publishing never blocks and
//! makes no request.

use std::sync::{Arc, OnceLock};

use oss_ext::lifecycle::Event;

/// Whatever takes events: the plugin dispatcher.
pub trait Events: Send + Sync {
    /// Offers `event`; never blocks.
    fn publish(&self, event: &Event);
}

/// A shared, fill-once handle to the event sink.
#[derive(Clone, Default)]
pub struct Bus(Arc<OnceLock<Arc<dyn Events>>>);

impl Bus {
    /// Sets where events go. Only the first call has an effect; whether it did.
    pub fn set(&self, events: Arc<dyn Events>) -> bool {
        self.0.set(events).is_ok()
    }

    /// Publishes one event of `kind` that happened to `index` at `at_ms`,
    /// for `tenant` (empty when it is the node's).
    pub fn publish(&self, kind: &str, tenant: &str, index: &str, at_ms: u64) {
        if let Some(events) = self.0.get() {
            events.publish(&Event {
                kind: kind.to_owned(),
                tenant: tenant.to_owned(),
                index: index.to_owned(),
                at_ms,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Rec(Mutex<Vec<Event>>);

    impl Events for Rec {
        fn publish(&self, event: &Event) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn an_event_before_anything_is_set_is_dropped_and_after_it_is_delivered() {
        let bus = Bus::default();
        bus.publish("index.created", "t", "a", 1);
        let rec = Arc::new(Rec::default());
        assert!(bus.set(rec.clone()));
        assert!(
            !bus.set(Arc::new(Rec::default())),
            "only the first set counts"
        );
        bus.clone().publish("index.deleted", "t", "a", 2);
        let got = rec.0.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, "index.deleted");
        assert_eq!(
            (got[0].tenant.as_str(), got[0].index.as_str(), got[0].at_ms),
            ("t", "a", 2)
        );
    }
}
