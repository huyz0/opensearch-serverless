//! The runner's backend over this node's own services: each action's effect
//! is the same code its route runs, with no request in between.

use std::sync::Arc;

use super::backend::Backend;
use crate::alias_route::AliasService;
use crate::index_delete::DeleteService;
use crate::settings_route::SettingsService;

/// The effects of the actions a node can run so far.
pub struct NodeBackend {
    settings: Arc<SettingsService>,
    deletes: Arc<DeleteService>,
    aliases: Arc<AliasService>,
}

impl NodeBackend {
    /// A backend whose block actions go through `settings`, whose delete
    /// goes through `deletes` and whose alias changes go through `aliases`.
    #[must_use]
    pub fn new(
        settings: Arc<SettingsService>,
        deletes: Arc<DeleteService>,
        aliases: Arc<AliasService>,
    ) -> Self {
        Self {
            settings,
            deletes,
            aliases,
        }
    }
}

impl Backend for NodeBackend {
    fn set_read_only(&self, index: &str, on: bool) -> Result<(), String> {
        self.settings.set_write_block(index, on)
    }

    fn set_replicas(&self, index: &str, n: u32) -> Result<(), String> {
        self.settings.set_replicas(index, n)
    }

    fn delete(&self, index: &str) -> Result<(), String> {
        self.deletes.delete_index(index)
    }

    fn set_alias(
        &self,
        index: &str,
        add: bool,
        alias: &str,
        write: Option<bool>,
    ) -> Result<(), String> {
        self.aliases.alter(index, add, alias, write)
    }
}
