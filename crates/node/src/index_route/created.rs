//! What a create tells the rest of the node: a hook called once an index
//! exists, for a feature that attaches something to every new index.

use std::sync::Arc;

use oss_rest::Request;

use super::IndexService;

/// Called with the request, the index name, its uuid and its creation time
/// in milliseconds, after the index was created. It cannot fail the create:
/// what it could not do it leaves for the feature's own routes.
pub type Created = Arc<dyn Fn(&mut Request<'_>, &str, &str, i64) + Send + Sync>;

impl IndexService {
    /// Calls `hook` after every create.
    #[must_use]
    pub fn with_created(mut self, hook: Created) -> Self {
        self.created = Some(hook);
        self
    }

    /// Publishes `index.created` to `bus` after every create.
    #[must_use]
    pub fn with_events(mut self, bus: crate::lifecycle_bus::Bus) -> Self {
        self.events = Some(bus);
        self
    }

    pub(super) fn notify_created(&self, req: &mut Request<'_>, index: &str, uuid: &str) {
        if let Some(bus) = &self.events {
            bus.publish(
                "index.created",
                req.principal().unwrap_or_default(),
                index,
                (self.clock)(),
            );
        }
        if let Some(hook) = &self.created {
            let now = i64::try_from((self.clock)()).unwrap_or(i64::MAX);
            hook(req, index, uuid, now);
        }
    }
}
