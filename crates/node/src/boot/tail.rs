//! The tail overlay in a booted node: the transport actions it answers are
//! listed before the server binds, and the handlers for them are given once
//! the roles exist.

use std::sync::{Arc, OnceLock};

use oss_cluster::LeaseClock;
use oss_cluster::Membership;
use oss_transport::actions::tail::{FETCH_ACTION, PUSH_ACTION};
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;
use oss_transport::server::{Handler, Handlers};

use super::support::Held;
use super::{BootError, NodeConfig, transport::Late};
use crate::assemble::Assembled;
use crate::tail_node::TailParts;
use crate::writer::RefreshVisibility;

type Slot = Arc<OnceLock<Arc<dyn Handler>>>;

/// Where the handlers of the tail actions are set once the roles exist.
pub(super) struct Slots {
    fetch: Slot,
    push: Slot,
}

impl Slots {
    /// Lists both actions in `handlers`, refusing until they are set. They
    /// are not behind the node's own signature check: what is set later is
    /// already behind one.
    pub(super) fn list(handlers: &mut Handlers) -> Self {
        let fetch = Late::default();
        let push = Late::default();
        let slots = Self {
            fetch: fetch.slot(),
            push: push.slot(),
        };
        handlers.insert(FETCH_ACTION.to_owned(), Arc::new(fetch));
        handlers.insert(PUSH_ACTION.to_owned(), Arc::new(push));
        slots
    }
}

/// Turns the tail overlay on when `config` says so and gives its handlers
/// to the transport server.
pub(super) fn wire(
    assembled: &mut Assembled,
    config: &NodeConfig,
    slots: &Slots,
    (client, auth): (Arc<Client>, Arc<Authenticator>),
    membership: &Arc<Membership>,
    clock: Arc<dyn LeaseClock>,
    store: Arc<dyn oss_blob::BlobStore>,
) -> Result<(), BootError> {
    let heads = oss_cluster::heads::ShardHeadStore::new(
        store,
        &config.base,
        Arc::clone(&clock),
        config.lease_ttl_millis,
        Arc::new(oss_cluster::LivenessOracle::new(Arc::clone(membership))),
    )
    .map_err(|e| BootError::Cluster(format!("tail overlay: {e}")))?;
    let parts = TailParts::over_transport(
        config.tail_overlay,
        &config.node_id,
        client,
        auth,
        Held(Arc::clone(membership)),
        clock,
    )
    .with_visibility(visibility(config.refresh_visibility))
    .with_unowned(Arc::new(move |index, shard| {
        // A head that is gone or unreadable is not known to be released.
        matches!(heads.read(index, shard), Ok(Some(head)) if head.owner_node_id.is_none())
    }));
    let Some(registry) = assembled
        .wire_tail(parts)
        .map_err(|e| BootError::Cluster(format!("tail overlay: {e:?}")))?
    else {
        return Ok(());
    };
    slots.fill(registry.into_handlers());
    Ok(())
}

/// The visibility mode `word` names; the default, `owner`, for any other.
pub(super) fn visibility(word: &str) -> RefreshVisibility {
    RefreshVisibility::parse(word).unwrap_or_default()
}

impl Slots {
    /// Sets the handler of each tail action in `handlers`.
    fn fill(&self, handlers: Handlers) {
        for (action, handler) in handlers {
            let slot = match action.as_str() {
                FETCH_ACTION => &self.fetch,
                PUSH_ACTION => &self.push,
                _ => continue,
            };
            slot.set(handler).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_transport::exception::RemoteException;
    use oss_transport::frame::VariableHeader;
    use oss_transport::server::Request;
    use oss_transport::stream::Output;
    use oss_transport::version::WireVersion;

    struct Answers(&'static [u8]);

    impl Handler for Answers {
        fn handle(&self, _: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
            out.bytes(self.0)
                .map_err(|_| RemoteException::rejected("full"))
        }
    }

    fn ask(handlers: &Handlers, action: &str) -> Result<Vec<u8>, RemoteException> {
        let variable = VariableHeader::default();
        let request = Request {
            action,
            variable: &variable,
            message: &[],
            version: WireVersion::SHELL,
        };
        let mut sink = Vec::new();
        handlers[action].handle(&request, &mut Output::new(&mut sink, 16))?;
        Ok(sink)
    }

    #[test]
    fn the_visibility_names_pick_their_mode_and_only_everywhere_makes_a_reader_check() {
        assert!(visibility("everywhere").grants());
        assert!(!visibility("serving").grants());
        assert!(visibility("serving").waits());
        assert!(!visibility("owner").waits());
        assert_eq!(visibility("nonsense"), RefreshVisibility::Owner);
    }

    #[test]
    fn both_tail_actions_are_listed_and_refuse_until_their_handlers_are_given() {
        let mut handlers = Handlers::new();
        let slots = Slots::list(&mut handlers);
        assert!(ask(&handlers, FETCH_ACTION).is_err());
        assert!(ask(&handlers, PUSH_ACTION).is_err());
        let mut given = Handlers::new();
        given.insert(FETCH_ACTION.to_owned(), Arc::new(Answers(b"f")));
        given.insert(PUSH_ACTION.to_owned(), Arc::new(Answers(b"p")));
        given.insert("other".to_owned(), Arc::new(Answers(b"o")));
        slots.fill(given);
        assert_eq!(ask(&handlers, FETCH_ACTION).expect("answers"), b"f");
        assert_eq!(ask(&handlers, PUSH_ACTION).expect("answers"), b"p");
        assert!(!handlers.contains_key("other"));
    }
}
