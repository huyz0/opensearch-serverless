//! `GET /_cat/indices` and `GET /_list/indices`: the indices a name, a list
//! or a prefix pattern stands for, a table of them.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/ListIndicesHandler.java`.
//! The table, the paging plan and the token are `oss_coordinator::cat`'s;
//! this reads the descriptors. A `_list` of one prefix is a walk, a page at a
//! time; everything else resolves under the pattern cap and is refused past
//! it rather than cut.

use std::collections::HashSet;
use std::sync::Arc;

use oss_coordinator::cat::indices::{self, Paging};
use oss_coordinator::cat::table::{Page, Table, unsupported};
use oss_format::IndexRecord;
use oss_meta::{DescriptorStore, MetaError, Target};
use oss_rest::{Method, Request, Response, RestError, RouteError, Router};

use crate::route_util::{flag, refuse, refused, send_table};

/// The most names a pattern may match.
const PATTERN_CAP: usize = 1_000;

/// The buffer a descriptor is read through: the most an index record is.
const DESCRIPTOR_BUFFER: usize = 1 << 20;

/// What a `_cat/indices` request takes: the table's options, and the hints
/// the shell reads and ignores.
const CAT_PARAMS: &[&str] = &[
    "format",
    "v",
    "h",
    "s",
    "bytes",
    "time",
    "help",
    "health",
    "pri",
    "expand_wildcards",
    "local",
    "master_timeout",
    "cluster_manager_timeout",
    "sort",
    "size",
    "next_token",
];

/// Serves the index tables over the descriptors.
pub struct CatService {
    descriptors: DescriptorStore,
}

fn unavailable(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyMatches { .. } => {
            // ALLOC: error path; the refusal's words.
            refuse(400, "too_many_indices", e.to_string())
        }
        // ALLOC: error path; the failure's words.
        _ => refuse(503, "metadata_unavailable", e.to_string()),
    }
}

fn row(table: &mut Table<6>, record: &IndexRecord<'_>) {
    let version = u64::try_from(record.mapping_version).unwrap_or(0);
    indices::row(
        table,
        &record.name,
        &record.uuid,
        record.number_of_shards,
        version,
    );
}

impl CatService {
    /// A service over `descriptors`.
    pub fn new(descriptors: DescriptorStore) -> Self {
        Self { descriptors }
    }

    /// `GET /_cat/indices[/{index}]` and `GET /_list/indices[/{index}]`.
    pub fn handle(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        listing: bool,
    ) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .map_or_else(|| "*".to_owned(), |i| i.into_owned());
        let columns = req.param("h");
        let sort = req.param("s");
        let refusal = unsupported(columns.as_deref(), sort.as_deref());
        let size = req.param("size");
        let token = req.param("next_token");
        let paging = indices::plan(listing, &index, size.as_deref(), token.as_deref(), refusal)
            .map_err(refused)?;
        let format = req.param("format");
        let verbose = flag(req, "v", false);
        req.body().reserve(DESCRIPTOR_BUFFER)?;
        // ALLOC: one buffer a request, DESCRIPTOR_BUFFER, reserved above.
        let mut buf = vec![0u8; DESCRIPTOR_BUFFER];
        let answer = match paging {
            Some(Paging {
                prefix: Some(prefix),
                after,
                size,
            }) => {
                let mut table = indices::table();
                let next = self
                    .descriptors
                    .list_page(&prefix, after.as_deref(), size, &mut buf, &mut |r| {
                        row(&mut table, r);
                    })
                    .map_err(|e| unavailable(&e))?;
                let token = next.map(|after| indices::token(&prefix, &after));
                let page = Page {
                    entity: "indices",
                    next_token: token.as_deref(),
                };
                table.answer(format.as_deref(), verbose, Some(page))
            }
            other => {
                let table = self.named(&index, &mut buf)?;
                let page = other.map(|_| Page {
                    entity: "indices",
                    next_token: None,
                });
                table.answer(format.as_deref(), verbose, page)
            }
        };
        send_table(resp, &answer)
    }

    /// The table of the indices `expression` (names, a list, prefix
    /// patterns) stands for.
    fn named(&self, expression: &str, buf: &mut [u8]) -> Result<Table<6>, RestError> {
        // ALLOC: one entry a name or match, bounded by the expression and the cap a pattern.
        let mut order: Vec<(String, bool)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for name in expression.split(',').map(str::trim) {
            if name.is_empty() {
                continue;
            }
            if let Some(prefix) = name.strip_suffix('*')
                && !prefix.contains('*')
            {
                let matched = self
                    .descriptors
                    .names_with_prefix(prefix, PATTERN_CAP)
                    .map_err(|e| unavailable(&e))?;
                for m in matched {
                    if seen.insert(m.clone()) {
                        order.push((m, true));
                    }
                }
            } else if name.contains(['*', '?']) {
                return Err(RestError::illegal_argument(format!(
                    "only a trailing-wildcard pattern is supported, and [{name}] is not one"
                )));
            } else if seen.insert(name.to_owned()) {
                order.push((name.to_owned(), false));
            }
        }
        let mut table = indices::table();
        for (name, from_pattern) in order {
            let found = self
                .descriptors
                .resolve(&name, buf)
                .map_err(|e| unavailable(&e))?;
            match found.target {
                Target::Index(record) => row(&mut table, &record),
                Target::Alias(alias) => {
                    // ALLOC: the members' names, bounded by the alias record.
                    let members: Vec<String> =
                        alias.indices.iter().map(|m| m.to_string()).collect();
                    for member in members {
                        if !seen.insert(member.clone()) {
                            continue;
                        }
                        if let Some(record) = self
                            .descriptors
                            .get(&member, buf)
                            .map_err(|e| unavailable(&e))?
                        {
                            row(&mut table, &record);
                        }
                    }
                }
                // A pattern filters what exists; a name asked for does not.
                Target::Absent if from_pattern => {}
                Target::Absent => {
                    // ALLOC: error path; the name.
                    return Err(refuse(
                        404,
                        "index_not_found",
                        format!("no such index: {name}"),
                    ));
                }
            }
        }
        Ok(table)
    }
}

/// The routes of the index tables.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<CatService>) -> Result<Router, RouteError> {
    let mut router = router;
    let table: [(&'static str, bool); 4] = [
        ("/_cat/indices", false),
        ("/_cat/indices/{index}", false),
        ("/_list/indices", true),
        ("/_list/indices/{index}", true),
    ];
    for (path, listing) in table {
        let service = Arc::clone(&service);
        router = router.route(
            Method::Get,
            path,
            CAT_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                service.handle(req, resp, listing)
            },
        )?;
    }
    Ok(router)
}
