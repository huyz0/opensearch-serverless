//! `POST /{index}/_rollover[/{new_index}]`: a new index behind a name, for an
//! alias or a data stream.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/indexadmin/RolloverHandler.java`.
//! The index is created first and the name moved second, by one compare-and-swap
//! of the alias record; a swap lost to a concurrent change removes the index
//! it made and answers 409, saying what became of it. Every name is literal.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use oss_coordinator::indices::admin::check_name;
use oss_coordinator::indices::create::CreateRequest;
use oss_coordinator::indices::rollover::{
    AliasFacts, Target, backing_index_name, check_target, evaluate, next_name, parse_body,
};
use oss_coordinator::route::resolve::Refusal;
use oss_format::AliasRecord;
use oss_meta::{AliasClock, AliasMutations, DescriptorStore, MetaError, PitClock, Target as Held};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::index_delete::DeleteService;
use crate::index_route::IndexService;
use crate::route_util::{read_body, refuse, refused};

mod count;
mod render;

pub use count::DocCount;
#[cfg(feature = "reader")]
pub use count::SearchCount;

/// The most bytes of a rollover body.
const MAX_BODY_BYTES: usize = 64 << 10;

/// Times a stream's next backing name is moved on when it is taken.
const NAME_ATTEMPTS: usize = 8;

/// Query parameters a rollover accepts and has nothing to do for.
const PARAMS: &[&str] = &[
    "dry_run",
    "timeout",
    "master_timeout",
    "cluster_manager_timeout",
    "wait_for_active_shards",
];

/// Serves the rollover routes over the descriptors.
pub struct RolloverService {
    index: Arc<IndexService>,
    deletes: Arc<DeleteService>,
    descriptors: DescriptorStore,
    clock: PitClock,
    buffer_bytes: usize,
    lease_ttl_millis: i64,
    counter: Option<Arc<dyn DocCount>>,
}

impl RolloverService {
    /// A service that creates the new index through `index` and takes it
    /// back out through `deletes` when the move is lost. A request holds
    /// register buffers of `buffer_bytes`, charged to the request-body pool,
    /// and an alias reservation lasts `lease_ttl_millis`.
    pub fn new(
        index: Arc<IndexService>,
        deletes: Arc<DeleteService>,
        descriptors: DescriptorStore,
        clock: PitClock,
        buffer_bytes: usize,
        lease_ttl_millis: i64,
    ) -> Self {
        Self {
            index,
            deletes,
            descriptors,
            clock,
            buffer_bytes,
            lease_ttl_millis,
            counter: None,
        }
    }

    /// Evaluates `max_docs` with `counter`; without one the condition is
    /// refused 501.
    #[must_use]
    pub fn with_counter(mut self, counter: Arc<dyn DocCount>) -> Self {
        self.counter = Some(counter);
        self
    }

    fn now(&self) -> i64 {
        i64::try_from((self.clock)()).unwrap_or(i64::MAX)
    }

    /// `POST /{index}/_rollover` and `POST /{index}/_rollover/{new_index}`.
    pub fn rollover(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let target = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("a rollover target is required"))?
            .into_owned();
        let new_index = req.path_param("new_index").map(Cow::into_owned);
        check_literal(&target)?;
        if let Some(new) = &new_index {
            check_name(new, true).map_err(refused)?;
        }
        let dry_run = req.param("dry_run").is_some_and(|v| v != "false");
        let body = read_body(req, MAX_BODY_BYTES, "the body is larger than 64 KiB")?;
        let ask = parse_body((!body.is_empty()).then_some(body.as_slice())).map_err(refused)?;
        let mut shape = ask.create.clone();
        let named_aliases = shape
            .as_mut()
            .map(|c| std::mem::take(&mut c.aliases))
            .unwrap_or_default();
        let mut body_aliases = Vec::with_capacity(named_aliases.len());
        for (name, props) in &named_aliases {
            body_aliases.push(crate::alias_route::body_alias(name, props)?);
        }
        // What the request holds, taken before anything is made so that
        // undoing the new index cannot be refused: the alias steps' three
        // buffers and the removal's. The reads before the create reuse the
        // removal's headroom, and the create reserves its own two.
        req.body().reserve(
            self.buffer_bytes
                .saturating_mul(3)
                .saturating_add(self.deletes.remove_bytes()),
        )?;
        let now = || self.now();
        let pause = |d: Duration| std::thread::sleep(d);
        let mut mutations = AliasMutations::new(
            &self.descriptors,
            AliasClock {
                now_millis: &now,
                pause: &pause,
            },
            self.lease_ttl_millis,
            self.buffer_bytes,
        );
        let at = mutations
            .resolve_for_mutation(&target)
            .map_err(lookup_failed)?;
        let facts = match &at {
            Some(at) => Target::Alias(AliasFacts {
                data_stream: at.alias.data_stream,
                mutation_pending: at.alias.mutation_pending(),
                write_index: at.alias.writes_to().map(str::to_owned),
            }),
            None if self.is_index(&target)? => Target::Index,
            None => Target::Missing,
        };
        check_target(&target, &facts, new_index.as_deref(), ask.shapes_index).map_err(refused)?;
        let Some(at) = at else {
            return Err(refuse(500, "rollover_failure", "the target vanished"));
        };
        let before = at.alias.clone();
        if before.data_stream && !body_aliases.is_empty() {
            return Err(refuse(
                400,
                "illegal_argument_exception",
                "aliases cannot be set in the body of a data stream rollover",
            ));
        }
        let old = before
            .writes_to()
            .map(str::to_owned)
            .ok_or_else(|| refuse(500, "rollover_failure", "the target has no write index"))?;
        let (created_at, shards) = self.old_index(&old)?;
        let generation = before.generation.checked_add(1).ok_or_else(|| {
            refuse(
                400,
                "illegal_argument_exception",
                // ALLOC: error path; one message.
                format!(
                    "rollover generation is exhausted at [{}]",
                    before.generation
                ),
            )
        })?;
        let mut new_name = match (&new_index, before.data_stream) {
            (Some(name), _) => name.clone(),
            (None, true) => backing_index_name(&target, generation),
            (None, false) => next_name(&old).map_err(refused)?.ok_or_else(|| {
                refuse(
                    400,
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "index name [{old}] does not match pattern '^.*-\\\\d+$'; name the new index in the path"
                    ),
                )
            })?,
        };
        let evaluation = evaluate(&ask.conditions, &old, created_at, self.now(), &mut || {
            self.counter
                .as_ref()
                .ok_or_else(max_docs_refusal)
                .and_then(|c| c.count(&old, shards))
        })
        .map_err(refused)?;
        if dry_run || !evaluation.should_roll {
            let body = render::response(&old, &new_name, &evaluation.met, dry_run, false);
            return resp.send(200, JSON, body.as_bytes());
        }
        let (created, new_generation) =
            self.make(req, &before, &target, &mut new_name, generation, shape)?;
        let mut after = before.clone();
        if before.data_stream {
            after.indices.push(Cow::Owned(new_name.clone()));
            after.generation = new_generation;
        } else if before.write_member.is_some() {
            // As core rolls an alias with a write index: the new index joins
            // it as the write index, and the old ones stay for reads.
            after.indices.push(Cow::Owned(new_name.clone()));
            after.write_member = Some(Cow::Owned(new_name.clone()));
        } else {
            // An alias of one index and no write index moves to the new one.
            after.indices = vec![Cow::Owned(new_name.clone())];
        }
        let moved = mutations
            .update_alias(&before, &after, at.generation)
            .map_err(lookup_failed);
        match moved {
            Ok(Some(_)) => {
                // The names the body asked for join the new index once the
                // move is published; one that cannot is told, the roll stays.
                let mut failed = None;
                for (alias, write) in &body_aliases {
                    if let Err(e) =
                        crate::alias_route::add_alias(&mut mutations, &new_name, alias, *write)
                    {
                        failed.get_or_insert_with(|| aliases_failed(&new_name, alias, &e));
                    }
                }
                if let Some(e) = failed {
                    return Err(e);
                }
            }
            Ok(None) => return Err(self.lost(&target, &new_name, &created)),
            Err(e) => {
                // The swap may have published before a later step failed:
                // the new index goes only when the record does not name it.
                let adopted = mutations
                    .resolve_for_mutation(&target)
                    .ok()
                    .flatten()
                    .is_none_or(|at| at.alias.indices.iter().any(|i| *i == new_name));
                if !adopted {
                    let _ = self.deletes.remove(&new_name, &created);
                }
                return Err(e);
            }
        }
        let body = render::response(&old, &new_name, &evaluation.met, false, true);
        resp.send(200, JSON, body.as_bytes())
    }

    /// Creates the index the name moves to: for a stream the next backing
    /// index, moved on by a generation while the name is taken, as core does.
    fn make(
        &self,
        req: &mut Request<'_>,
        before: &AliasRecord<'_>,
        target: &str,
        name: &mut String,
        mut generation: i64,
        shape: Option<CreateRequest>,
    ) -> Result<(String, i64), RestError> {
        if !before.data_stream {
            return self
                .index
                .create_rolled(req, name, shape)
                .map(|uuid| (uuid, 0));
        }
        let timestamp = before.timestamp_field.as_deref().unwrap_or("@timestamp");
        for attempt in 0..=NAME_ATTEMPTS {
            let held = req.body().reserved();
            match self.index.create_next_backing(req, name, target, timestamp) {
                Ok(uuid) => return Ok((uuid, generation)),
                Err(e) if is_taken(&e) && attempt < NAME_ATTEMPTS => {
                    // The failed attempt's buffer is gone: give it back, so
                    // moving past names holds no more than one create.
                    let extra = req.body().reserved().saturating_sub(held);
                    req.body().release(extra);
                    generation = generation.checked_add(1).ok_or_else(|| {
                        refuse(
                            400,
                            "illegal_argument_exception",
                            "data stream generation is exhausted",
                        )
                    })?;
                    *name = backing_index_name(target, generation);
                }
                Err(e) => return Err(e),
            }
        }
        Err(refuse(
            500,
            "rollover_failure",
            "no free backing index name",
        ))
    }

    /// The 409 for a move lost to a concurrent change, after taking the new
    /// index back out, saying what became of it.
    fn lost(&self, target: &str, new_name: &str, uuid: &str) -> RestError {
        let left = match self.deletes.remove(new_name, uuid) {
            Ok(true) => format!("the index [{new_name}] created for it was removed"),
            Ok(false) => format!(
                "the index [{new_name}] changed concurrently and was not removed by this rollback"
            ),
            Err(e) => format!(
                "the index [{new_name}] was created and not adopted, and could not be removed: {e}"
            ),
        };
        refuse(
            409,
            "version_conflict_engine_exception",
            // ALLOC: error path; one message.
            format!(
                "[{target}] was changed concurrently; nothing was rolled over and {left}; retry"
            ),
        )
    }

    /// Whether `name` holds an index: one read.
    fn is_index(&self, name: &str) -> Result<bool, RestError> {
        // ALLOC: one buffer, only for a target that is no alias.
        let mut buf = vec![0u8; self.buffer_bytes];
        Ok(matches!(
            self.descriptors
                .resolve(name, &mut buf)
                .map_err(lookup_failed)?
                .target,
            Held::Index(_)
        ))
    }

    /// When the index `name` was created, in milliseconds, and its shard
    /// count; 404 when it has no descriptor.
    fn old_index(&self, name: &str) -> Result<(i64, u32), RestError> {
        // ALLOC: one buffer a rollover.
        let mut buf = vec![0u8; self.buffer_bytes];
        match self
            .descriptors
            .get(name, &mut buf)
            .map_err(lookup_failed)?
        {
            Some(record) => Ok((record.created_at_millis, record.number_of_shards)),
            None => Err(refuse(
                404,
                "index_not_found",
                // ALLOC: error path; one message.
                format!("the current index [{name}] does not exist"),
            )),
        }
    }
}

/// The error for a body alias that could not be added after the roll.
fn aliases_failed(new_name: &str, alias: &str, cause: &RestError) -> RestError {
    let reason = cause
        .as_exception()
        .map_or_else(|| "failed".to_owned(), |e| e.reason().to_owned());
    refuse(
        cause.status(),
        "rollover_alias_failure",
        // ALLOC: error path; one message.
        format!(
            "rolled over to [{new_name}] but adding it to alias [{alias}] failed: {reason}; add it with POST /_aliases"
        ),
    )
}

fn is_taken(e: &RestError) -> bool {
    e.as_exception()
        .is_some_and(|x| x.kind() == "resource_already_exists_exception")
}

fn max_docs_refusal() -> Refusal {
    Refusal::new(
        501,
        "unsupported_condition",
        "max_docs cannot be evaluated: this node has no search to count the index with",
    )
}

fn lookup_failed(e: MetaError) -> RestError {
    // ALLOC: error path; one message.
    refuse(500, "rollover_failure", e.to_string())
}

/// A target name that is no pattern, list or reserved word.
fn check_literal(name: &str) -> Result<(), RestError> {
    if name.contains(['*', ',']) || name == "_all" {
        return Err(refuse(
            501,
            "unsupported_rollover_target",
            // ALLOC: error path; one message.
            format!("[{name}] is a pattern or a list: a rollover target is a literal name"),
        ));
    }
    check_name(name, false).map_err(refused)
}

/// The rollover routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<RolloverService>) -> Result<Router, RouteError> {
    let plain = Arc::clone(&service);
    router
        .route(
            Method::Post,
            "/{index}/_rollover",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| plain.rollover(req, resp),
        )?
        .route(
            Method::Post,
            "/{index}/_rollover/{new_index}",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.rollover(req, resp),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_taken_is_told_by_its_kind() {
        assert!(is_taken(&refuse(
            400,
            "resource_already_exists_exception",
            "x"
        )));
        assert!(!is_taken(&refuse(400, "illegal_argument_exception", "x")));
    }
}
