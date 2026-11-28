//! Alias routes: add, remove and read the aliases of named indices.
//!
//! Every name is literal: a pattern, `_all` or a comma list would need a
//! listing, and a request's cost never depends on how many aliases exist. A
//! change goes through [`AliasMutations`], whose steps are all compare-and-swap.

use std::sync::Arc;
use std::time::Duration;

use crate::metadata_block;
use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_format::AliasRecord;
use oss_meta::{AliasClock, AliasMutations, DescriptorStore, MetaError, PitClock, Target};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::read_body;

mod change;
mod parse;
pub(crate) use change::add_alias;
use change::apply;
use parse::{Change, literal, parse_actions, refuse, unsupported, write_flag};

/// The most bytes of a change body.
const MAX_BODY_BYTES: usize = 64 << 10;

/// Query parameters a change accepts and has nothing to do for.
const CHANGE_PARAMS: &[&str] = &["timeout", "master_timeout", "cluster_manager_timeout"];

const ACK: &[u8] = br#"{"acknowledged":true}"#;

/// Serves the alias routes over the descriptors.
pub struct AliasService {
    descriptors: DescriptorStore,
    clock: PitClock,
    buffer_bytes: usize,
    lease_ttl_millis: i64,
}

impl AliasService {
    /// A service over `descriptors`, telling the time by `clock`
    /// (milliseconds). A request holds register buffers of `buffer_bytes`,
    /// charged to the request-body pool, and a mutation's reservation lasts
    /// `lease_ttl_millis`.
    pub fn new(
        descriptors: DescriptorStore,
        clock: PitClock,
        buffer_bytes: usize,
        lease_ttl_millis: i64,
    ) -> Self {
        Self {
            descriptors,
            clock,
            buffer_bytes,
            lease_ttl_millis,
        }
    }

    /// Refuses when any of `indices` holds the metadata block.
    fn unblocked(&self, indices: &[&str]) -> Result<(), RestError> {
        metadata_block::check_all(&self.descriptors, indices, self.buffer_bytes)
    }

    fn mutate<R>(
        &self,
        req: &mut Request<'_>,
        run: impl FnOnce(&mut AliasMutations<'_>) -> Result<R, RestError>,
    ) -> Result<R, RestError> {
        req.body().reserve(self.buffer_bytes.saturating_mul(3))?;
        self.mutate_unreserved(run)
    }

    fn mutate_unreserved<R>(
        &self,
        run: impl FnOnce(&mut AliasMutations<'_>) -> Result<R, RestError>,
    ) -> Result<R, RestError> {
        let now = || i64::try_from((self.clock)()).unwrap_or(i64::MAX);
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
        run(&mut mutations)
    }

    /// Adds `index` to `alias` or removes it, as one action of
    /// `POST /_aliases` does, for a caller that holds no request.
    ///
    /// # Errors
    ///
    /// The reason the change was refused.
    pub fn alter(
        &self,
        index: &str,
        add: bool,
        alias: &str,
        write: Option<bool>,
    ) -> Result<(), String> {
        let named = |e: RestError| {
            e.as_exception().map_or_else(
                || "the alias change failed".to_owned(),
                |x| x.reason().to_owned(),
            )
        };
        parse::check_literal(index).map_err(named)?;
        parse::check_literal(alias).map_err(named)?;
        let change = Change {
            add,
            index: index.to_owned(),
            alias: alias.to_owned(),
            write,
        };
        self.mutate_unreserved(|m| {
            self.unblocked(&[index])?;
            apply(m, &change)
        })
        .map_err(named)
    }

    /// `PUT|POST /{index}/_alias/{name}`.
    pub fn put(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let (index, alias) = names(req)?;
        let body = read_body(req, MAX_BODY_BYTES, "the body is larger than 64 KiB")?;
        let write = if body.iter().all(u8::is_ascii_whitespace) {
            None
        } else {
            let parsed = parse_json(&body)
                .map_err(|e| RestError::illegal_argument(format!("invalid body: {e}")))?;
            let Json::Obj(object) = parsed else {
                return Err(RestError::illegal_argument("the body must be an object"));
            };
            write_flag(&object)?
        };
        let change = Change {
            add: true,
            index,
            alias,
            write,
        };
        self.mutate(req, |m| {
            self.unblocked(&[change.index.as_str()])?;
            apply(m, &change)
        })?;
        resp.send(200, JSON, ACK)
    }

    /// `DELETE /{index}/_alias/{name}`.
    pub fn delete(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let (index, alias) = names(req)?;
        let change = Change {
            add: false,
            index,
            alias,
            write: None,
        };
        self.mutate(req, |m| {
            self.unblocked(&[change.index.as_str()])?;
            apply(m, &change)
        })?;
        resp.send(200, JSON, ACK)
    }

    /// `POST /_aliases`.
    pub fn actions(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let body = read_body(req, MAX_BODY_BYTES, "the body is larger than 64 KiB")?;
        let changes = parse_actions(&body)?;
        let named: Vec<&str> = changes.iter().map(|c| c.index.as_str()).collect();
        self.mutate(req, |m| {
            self.unblocked(&named)?;
            for change in &changes {
                apply(m, change)?;
            }
            Ok(())
        })?;
        resp.send(200, JSON, ACK)
    }

    /// `GET|HEAD /_alias/{name}`.
    pub fn read_alias(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let alias = literal(req, "name")?;
        match self.members(req, &alias)? {
            Some(record) => resp.send(200, JSON, render(&record).as_bytes()),
            None => missing(resp, &alias),
        }
    }

    /// `GET /{index}/_alias` and `GET /{index}/_alias/{name}`.
    pub fn read_index(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let index = literal(req, "index")?;
        let only = match req.path_param("name") {
            Some(_) => Some(literal(req, "name")?),
            None => None,
        };
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one buffer a request, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let hinted: Vec<String> = match self
            .descriptors
            .resolve(&index, &mut buf)
            .map_err(read_failed)?
            .target
        {
            Target::Index(record) => record.aliased_by.iter().map(|n| n.to_string()).collect(),
            Target::Alias(_) | Target::Absent => {
                return Err(refuse(
                    404,
                    "index_not_found_exception",
                    format!("no such index [{index}]"),
                ));
            }
        };
        let mut aliases = String::new();
        for name in hinted
            .iter()
            .filter(|n| only.as_deref().is_none_or(|o| o == n.as_str()))
        {
            if let Some(record) = self.members(req, name)?
                && !record.data_stream
                && record.indices.iter().any(|i| i == &index)
            {
                let write = record.write_index() == Some(index.as_str());
                push_alias(&mut aliases, name, write);
            }
        }
        if let Some(alias) = &only
            && aliases.is_empty()
        {
            return missing(resp, alias);
        }
        let body = format!("{{{}:{{\"aliases\":{{{aliases}}}}}}}", quoted(&index));
        resp.send(200, JSON, body.as_bytes())
    }

    /// The published record of `alias`, one register read.
    fn members(
        &self,
        req: &mut Request<'_>,
        alias: &str,
    ) -> Result<Option<AliasRecord<'static>>, RestError> {
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one buffer a read, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let held = self
            .descriptors
            .resolve(alias, &mut buf)
            .map_err(read_failed)?;
        Ok(match held.target {
            Target::Alias(record) => Some(record.into_owned()),
            Target::Index(_) | Target::Absent => None,
        })
    }
}

fn read_failed(e: MetaError) -> RestError {
    refuse(500, "alias_lookup_failure", e.to_string())
}

fn missing(resp: &mut Response<'_>, alias: &str) -> Result<(), RestError> {
    let reason = quoted(&format!("alias [{alias}] missing"));
    let body = format!("{{\"error\":{reason},\"status\":404}}");
    resp.send(404, JSON, body.as_bytes())
}

/// `s` as a JSON string.
fn quoted(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len().saturating_add(2));
    // Writing to a Vec cannot fail.
    let _ = oss_rest::write_json_str(&mut out, s);
    String::from_utf8_lossy(&out).into_owned()
}

fn push_alias(out: &mut String, name: &str, write: bool) {
    if !out.is_empty() {
        out.push(',');
    }
    out.push_str(&quoted(name));
    out.push_str(if write {
        ":{\"is_write_index\":true}"
    } else {
        ":{}"
    });
}

/// `{"<index>":{"aliases":{"<alias>":{..}}}}` for each member of `record`.
fn render(record: &AliasRecord<'_>) -> String {
    let write = record.write_index();
    let mut members: Vec<&str> = record.indices.iter().map(AsRef::as_ref).collect();
    members.sort_unstable();
    let mut out = String::from("{");
    for (n, index) in members.iter().enumerate() {
        if n > 0 {
            out.push(',');
        }
        let mut alias = String::new();
        push_alias(&mut alias, &record.name, write == Some(*index));
        out.push_str(&format!("{}:{{\"aliases\":{{{alias}}}}}", quoted(index)));
    }
    out.push('}');
    out
}

fn names(req: &Request<'_>) -> Result<(String, String), RestError> {
    Ok((literal(req, "index")?, literal(req, "name")?))
}

/// What an alias named in a rollover body asks: its name checked as a
/// literal and its write flag read, other keys refused as an alias change
/// refuses them.
pub(crate) fn body_alias(
    name: &str,
    props: &[(String, Json)],
) -> Result<(String, Option<bool>), RestError> {
    parse::check_literal(name)?;
    Ok((name.to_owned(), write_flag(props)?))
}

/// The alias routes.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<AliasService>) -> Result<Router, RouteError> {
    let (put, post, del) = (
        Arc::clone(&service),
        Arc::clone(&service),
        Arc::clone(&service),
    );
    let (acts, one, head, idx, idx_one) = (
        Arc::clone(&service),
        Arc::clone(&service),
        Arc::clone(&service),
        Arc::clone(&service),
        service,
    );
    router
        .route(
            Method::Put,
            "/{index}/_alias/{name}",
            CHANGE_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| put.put(req, resp),
        )?
        .route(
            Method::Post,
            "/{index}/_alias/{name}",
            CHANGE_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| post.put(req, resp),
        )?
        .route(
            Method::Delete,
            "/{index}/_alias/{name}",
            CHANGE_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| del.delete(req, resp),
        )?
        .route(
            Method::Post,
            "/_aliases",
            CHANGE_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| acts.actions(req, resp),
        )?
        .route(
            Method::Get,
            "/_alias",
            &[],
            move |_req: &mut Request<'_>, _resp: &mut Response<'_>| {
                Err(unsupported(
                    "listing every alias needs a listing: name the alias or the index",
                ))
            },
        )?
        .route(
            Method::Get,
            "/_alias/{name}",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| one.read_alias(req, resp),
        )?
        .route(
            Method::Head,
            "/_alias/{name}",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| head.read_alias(req, resp),
        )?
        .route(
            Method::Get,
            "/{index}/_alias",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| idx.read_index(req, resp),
        )?
        .route(
            Method::Get,
            "/{index}/_alias/{name}",
            &[],
            move |req: &mut Request<'_>, resp: &mut Response<'_>| idx_one.read_index(req, resp),
        )
}
