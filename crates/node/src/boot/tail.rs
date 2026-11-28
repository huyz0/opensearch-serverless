//! The tail overlay in a booted node: the transport actions it answers are
//! listed before the server binds, and the handlers for them are given once
//! the roles exist.

use std::sync::{Arc, OnceLock};

use oss_cluster::LeaseClock;
use oss_cluster::Membership;
use oss_transport::actions::tail::{FETCH_ACTION, PUSH_ACTION};
use oss_transport::actions::warm::{HOT_SET_ACTION, PAGES_ACTION};
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;
use oss_transport::server::{Handler, Handlers};

use super::support::Held;
use super::{BootError, NodeConfig, transport::Late};
use crate::assemble::Assembled;
use crate::handlers::Registry;
use crate::lifecycle::Readiness;
use crate::tail_node::TailParts;
use crate::warm_handlers::{Gate, HotSetHandler, Limits, PagesHandler};
use crate::writer::RefreshVisibility;

mod joining;

use joining::Joining;

type Slot = Arc<OnceLock<Arc<dyn Handler>>>;

/// Where the handlers of the tail actions are set once the roles exist.
pub(super) struct Slots {
    fetch: Slot,
    push: Slot,
    hot_set: Slot,
    pages: Slot,
}

impl Slots {
    /// Lists the tail and warm-copy actions in `handlers`, refusing until
    /// they are set. They are not behind the node's own signature check:
    /// what is set later is already behind one.
    pub(super) fn list(handlers: &mut Handlers) -> Self {
        let (fetch, push) = (Late::default(), Late::default());
        let (hot_set, pages) = (Late::default(), Late::default());
        let slots = Self {
            fetch: fetch.slot(),
            push: push.slot(),
            hot_set: hot_set.slot(),
            pages: pages.slot(),
        };
        handlers.insert(FETCH_ACTION.to_owned(), Arc::new(fetch));
        handlers.insert(PUSH_ACTION.to_owned(), Arc::new(push));
        handlers.insert(HOT_SET_ACTION.to_owned(), Arc::new(hot_set));
        handlers.insert(PAGES_ACTION.to_owned(), Arc::new(pages));
        slots
    }
}

/// Turns the tail overlay on when `config` says so and gives its handlers
/// to the transport server.
pub(super) fn wire(
    assembled: &mut Assembled,
    config: &NodeConfig,
    slots: &Slots,
    (client, auth, readiness): (&Arc<Client>, &Arc<Authenticator>, &Arc<Readiness>),
    membership: &Arc<Membership>,
    clock: Arc<dyn LeaseClock>,
    store: Arc<dyn oss_blob::BlobStore>,
) -> Result<(), BootError> {
    // The acknowledged mark the overlay reads moves with the pipeline.
    let depth = usize::try_from(config.write_pipeline_depth).unwrap_or(1);
    assembled.role.set_pipeline_depth(depth);
    install_warm(assembled, config, slots, auth, membership)?;
    join_warm(assembled, config, (client, auth), membership, readiness);
    let heads = oss_cluster::heads::ShardHeadStore::new(
        store,
        &config.base,
        Arc::clone(&clock),
        config.lease_ttl_millis,
        Arc::new(oss_cluster::LivenessOracle::new(Arc::clone(membership))),
    )
    .map_err(|e| BootError::Cluster(format!("tail overlay: {e}")))?;
    let heads = Arc::new(heads);
    let owners = Arc::clone(&heads);
    let parts = TailParts::over_transport(
        config.tail_overlay,
        &config.node_id,
        Arc::clone(client),
        Arc::clone(auth),
        Held(Arc::clone(membership)),
        clock,
    )
    .with_visibility(visibility(config.refresh_visibility))
    .with_unowned(Arc::new(move |index, shard| {
        // A head that is gone or unreadable is not known to be released.
        matches!(heads.read(index, shard), Ok(Some(head)) if head.owner_node_id.is_none())
    }))
    .with_owner(Arc::new(move |index, shard| {
        match owners.read(index, shard) {
            Ok(Some(head)) => head.owner_node_id,
            _ => None,
        }
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

/// Answers a joining reader's warm-copy asks when `config` asks for it:
/// the handlers sit behind the node's signature check like the tail's.
fn install_warm(
    assembled: &Assembled,
    config: &NodeConfig,
    slots: &Slots,
    auth: &Arc<Authenticator>,
    membership: &Arc<Membership>,
) -> Result<(), BootError> {
    let Some(warm) = config.page_cache.as_ref().and_then(|tier| tier.warm) else {
        return Ok(());
    };
    let cache = Arc::clone(assembled.search.page_cache());
    let limits = Limits {
        width: usize::try_from(warm.width).unwrap_or(usize::MAX),
        max_shards: usize::try_from(warm.max_shards).unwrap_or(usize::MAX),
        max_bytes: warm.max_bytes,
        max_concurrent: usize::try_from(warm.max_concurrent).unwrap_or(usize::MAX),
    };
    let gate = Arc::new(Gate::with_rate(warm.serve_bytes_per_second));
    let mut registry = Registry::new(Arc::clone(auth));
    registry
        .add(
            HOT_SET_ACTION,
            HotSetHandler::new(
                Arc::clone(&cache),
                Arc::clone(&assembled.search),
                Held(Arc::clone(membership)),
                limits,
                Arc::clone(&gate),
            ),
        )
        .and_then(|()| {
            registry.add(
                PAGES_ACTION,
                PagesHandler::new(cache, Arc::clone(&assembled.search), limits, gate),
            )
        })
        .map_err(|e| BootError::Cluster(format!("warm copy: {e:?}")))?;
    slots.fill(registry.into_handlers());
    Ok(())
}

/// Warms this node from its peers on a thread of its own, then lets it in:
/// the node answers 503 on ready and holds no `search` role until it holds
/// `ready_percent` of the bytes its peers named or `max_wait` passes. A node
/// not asked to warm is already ready.
fn join_warm(
    assembled: &Assembled,
    config: &NodeConfig,
    (client, auth): (&Arc<Client>, &Arc<Authenticator>),
    membership: &Arc<Membership>,
    readiness: &Arc<Readiness>,
) {
    let Some(warm) = config.page_cache.as_ref().and_then(|tier| tier.warm) else {
        return;
    };
    let job = Joining {
        warm,
        node_id: config.node_id.clone(),
        client: Arc::clone(client),
        auth: Arc::clone(auth),
        membership: Arc::clone(membership),
        search: Arc::clone(&assembled.search),
        budget: assembled.budget.clone(),
        readiness: Arc::clone(readiness),
    };
    // A node that cannot start the thread serves cold rather than never
    // becoming ready.
    if std::thread::Builder::new()
        .name("warm-join".to_owned())
        .spawn(move || job.run())
        .is_err()
    {
        readiness.finish_warming();
    }
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
                HOT_SET_ACTION => &self.hot_set,
                PAGES_ACTION => &self.pages,
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
    fn both_warm_actions_are_listed_and_refuse_until_their_handlers_are_given() {
        let mut handlers = Handlers::new();
        let slots = Slots::list(&mut handlers);
        assert!(ask(&handlers, HOT_SET_ACTION).is_err());
        assert!(ask(&handlers, PAGES_ACTION).is_err());
        let mut given = Handlers::new();
        given.insert(HOT_SET_ACTION.to_owned(), Arc::new(Answers(b"h")));
        given.insert(PAGES_ACTION.to_owned(), Arc::new(Answers(b"p")));
        slots.fill(given);
        assert_eq!(ask(&handlers, HOT_SET_ACTION).expect("answers"), b"h");
        assert_eq!(ask(&handlers, PAGES_ACTION).expect("answers"), b"p");
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
