//! The runner's backend over this node's own services: each action's effect
//! is the same code its route runs, with no request in between.

use std::sync::Arc;

use super::backend::Backend;
use crate::alias_route::AliasService;
use crate::index_delete::DeleteService;
use crate::notifications_route::{Delivery, Notifier};
use crate::settings_route::SettingsService;

/// The effects of the actions a node can run so far.
pub struct NodeBackend {
    settings: Arc<SettingsService>,
    deletes: Arc<DeleteService>,
    aliases: Arc<AliasService>,
    notifier: Arc<Notifier>,
}

impl NodeBackend {
    /// A backend whose block actions go through `settings`, whose delete
    /// goes through `deletes`, whose alias changes go through `aliases` and
    /// whose notifications go through `notifier`.
    #[must_use]
    pub fn new(
        settings: Arc<SettingsService>,
        deletes: Arc<DeleteService>,
        aliases: Arc<AliasService>,
        notifier: Arc<Notifier>,
    ) -> Self {
        Self {
            settings,
            deletes,
            aliases,
            notifier,
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

    fn notify(&self, channel: &str, text: &str) -> Result<(), String> {
        delivered(channel, &self.notifier.send(channel, text))
    }
}

/// A delivery as a step's outcome: any 2xx is sent, anything else fails
/// naming the channel and the status the notifier reported.
fn delivered(channel: &str, sent: &Delivery) -> Result<(), String> {
    if (200..300).contains(&sent.status_code) {
        Ok(())
    } else {
        Err(format!(
            "the notification to channel [{channel}] failed: {} {}",
            sent.status_code, sent.status_text
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_2xx_delivery_is_a_sent_notification() {
        let at = |code: u16, text: &str| Delivery {
            status_code: code,
            status_text: text.to_owned(),
        };
        assert_eq!(delivered("c1", &at(200, "OK")), Ok(()));
        assert_eq!(
            delivered("c1", &at(404, "not found")).unwrap_err(),
            "the notification to channel [c1] failed: 404 not found"
        );
    }
}
