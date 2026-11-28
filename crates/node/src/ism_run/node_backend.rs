//! The runner's backend over this node's own services: each action's effect
//! is the same code its route runs, with no request in between.

use std::sync::Arc;

use super::backend::Backend;
use crate::settings_route::SettingsService;

/// The effects of the actions a node can run so far.
pub struct NodeBackend {
    settings: Arc<SettingsService>,
}

impl NodeBackend {
    /// A backend whose block actions go through `settings`.
    #[must_use]
    pub fn new(settings: Arc<SettingsService>) -> Self {
        Self { settings }
    }
}

impl Backend for NodeBackend {
    fn set_read_only(&self, index: &str, on: bool) -> Result<(), String> {
        self.settings.set_write_block(index, on)
    }
}
