//! The mapping routes of index admin: read an index's mapping back and add
//! fields to it.
//!
//! Ported from `MappingUpdateHandler` and the `_mapping` routes of
//! `IndexAdminHandler` (shell 429b4cbc): one concrete index, over the
//! descriptor register, with `oss-coordinator`'s plan for the merge.

use std::borrow::Cow;
use std::sync::Arc;

use oss_coordinator::indices::admin::check_name;
use oss_coordinator::indices::mapping_update::{ATTEMPTS, MappingPlan, plan};
use oss_meta::{MetaError, Target};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router};

use super::{IndexService, refusal};
use crate::route_util::{read_body, refuse};

/// The most bytes of a mapping update body.
const MAX_BODY_BYTES: usize = 1 << 20;

/// The query parameters a mapping read accepts and has nothing to do for:
/// one node, no replicas, no closed or hidden indices, nothing to wait for.
const GET_PARAMS: &[&str] = &[
    "local",
    "include_defaults",
    "ignore_unavailable",
    "allow_no_indices",
    "expand_wildcards",
    "master_timeout",
    "cluster_manager_timeout",
    "timeout",
    "wait_for_active_shards",
    "include_type_name",
    "flat_settings",
];

/// The same for an update.
const PUT_PARAMS: &[&str] = &[
    "write_index_only",
    "timeout",
    "master_timeout",
    "cluster_manager_timeout",
    "ignore_unavailable",
    "allow_no_indices",
    "expand_wildcards",
    "include_type_name",
];

/// The acknowledgement of an update.
const ACKNOWLEDGED: &[u8] = br#"{"acknowledged":true}"#;

fn not_found(index: &str) -> RestError {
    refuse(
        404,
        "index_not_found",
        // ALLOC: error path; one message.
        format!("no such index: {index}"),
    )
}

impl IndexService {
    /// What `name`'s register holds, with a name no index may have (a list,
    /// a wildcard) as an absent one: not an index, not a failure.
    fn held<'b>(&self, name: &str, buf: &'b mut [u8]) -> Result<(u64, Target<'b>), RestError> {
        match self.descriptors.resolve(name, buf) {
            Ok(held) => Ok((held.generation, held.target)),
            Err(MetaError::InvalidName(_)) => Ok((0, Target::Absent)),
            Err(e) => Err(refuse(500, "index_lookup_failure", e.to_string())),
        }
    }

    /// `GET /{index}/_mapping` and `/_mappings`: the stored mapping of one
    /// concrete index, as the Java renders its descriptor's mapping.
    pub fn mapping(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        check_name(&index, false).map_err(refusal)?;
        req.body().reserve(self.buffer_bytes)?;
        // ALLOC: one buffer a request, buffer_bytes, reserved above.
        let mut buf = vec![0u8; self.buffer_bytes];
        let (_, target) = self.held(&index, &mut buf)?;
        let Target::Index(record) = target else {
            return Err(not_found(&index));
        };
        let mapping = record.mapping.as_deref().unwrap_or("{}");
        // ALLOC: the answer, the name and the stored mapping, bounded by the descriptor buffer.
        let mut body: Vec<u8> = Vec::with_capacity(mapping.len().saturating_add(64));
        body.push(b'{');
        oss_rest::write_json_str(&mut body, &record.name)
            .map_err(|e| refuse(500, "index_lookup_failure", e.to_string()))?;
        body.extend_from_slice(b":{\"mappings\":");
        body.extend_from_slice(mapping.as_bytes());
        body.extend_from_slice(b"}}");
        resp.send(200, JSON, &body)
    }

    /// `PUT|POST /{index}/_mapping` and `/_mappings`: adds the body's fields
    /// to the stored mapping of one concrete index.
    pub fn put_mapping(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        check_name(&index, false).map_err(refusal)?;
        let body = read_body(req, MAX_BODY_BYTES, "the mapping body is too large")?;
        req.body().reserve(self.buffer_bytes)?;
        self.update_mapping(&index, &body, &|_| {})?;
        resp.send(200, JSON, ACKNOWLEDGED)
    }

    /// Plans and swaps the merge of `body` into `index`'s mapping, again
    /// when the swap is lost; `between` runs after each read and before its
    /// swap, with the attempt's number.
    pub(super) fn update_mapping(
        &self,
        index: &str,
        body: &[u8],
        between: &dyn Fn(u32),
    ) -> Result<(), RestError> {
        if body.is_empty() {
            return Err(refuse(400, "missing_body", "a mapping body is required"));
        }
        for attempt in 0..ATTEMPTS {
            // ALLOC: one buffer an attempt, buffer_bytes, as a read's.
            let mut buf = vec![0u8; self.buffer_bytes];
            let (generation, target) = self.held(index, &mut buf)?;
            let Target::Index(record) = target else {
                return Err(not_found(index));
            };
            let merged = match plan(record.mapping.as_deref(), Some(body)).map_err(refusal)? {
                MappingPlan::Unchanged => return Ok(()),
                MappingPlan::Replace(text) => text,
            };
            between(attempt);
            let mut updated = record.clone();
            let slack = 8 << 10;
            updated.mapping = Some(Cow::Owned(merged));
            updated.mapping_version = updated.mapping_version.saturating_add(1);
            let size = updated
                .mapping
                .as_deref()
                .map_or(0, str::len)
                .saturating_mul(2)
                .saturating_add(self.buffer_bytes)
                .saturating_add(slack);
            // ALLOC: the record written, bounded by the mapping and the descriptor buffer.
            let mut out = vec![0u8; size];
            let swapped = self
                .descriptors
                .update(&updated, generation, &mut out)
                .map_err(|e| refuse(500, "index_update_failure", e.to_string()))?;
            if swapped.is_some() {
                return Ok(());
            }
        }
        Err(refuse(
            409,
            "status_exception",
            // ALLOC: error path; one message.
            format!("the mapping for {index} is being changed concurrently; retry"),
        ))
    }
}

/// Registers the mapping routes on `router`.
pub(super) fn routes(router: Router, service: &Arc<IndexService>) -> Result<Router, RouteError> {
    let mut router = router;
    for path in ["/{index}/_mapping", "/{index}/_mappings"] {
        let read = Arc::clone(service);
        router = router.route(
            Method::Get,
            path,
            GET_PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| read.mapping(req, resp),
        )?;
        for method in [Method::Put, Method::Post] {
            let write = Arc::clone(service);
            router = router.route(
                method,
                path,
                PUT_PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| write.put_mapping(req, resp),
            )?;
        }
    }
    Ok(router)
}

#[cfg(test)]
mod tests {
    //! docs/specs/node-index-mapping-put AC-5: a lost swap.

    use std::sync::Arc;

    use oss_blob::{BlobStore, Emulator};
    use oss_meta::{DeleteMode, DescriptorStore, PitClock};

    use super::*;

    const BUF: usize = 1 << 16;

    fn store() -> Arc<Emulator> {
        Arc::new(Emulator::new(1 << 24, 5))
    }

    fn descriptors(store: &Arc<Emulator>) -> DescriptorStore {
        DescriptorStore::new(
            Arc::clone(store) as Arc<dyn BlobStore>,
            "",
            DeleteMode::Conditional,
        )
        .expect("names")
    }

    fn service(store: &Arc<Emulator>) -> IndexService {
        let clock: PitClock = Arc::new(|| 1);
        IndexService::new(descriptors(store), clock, BUF)
    }

    fn create(store: &Arc<Emulator>) {
        let mut record = oss_format::IndexRecord::new("logs", "u1", 1);
        record.mapping = Some(Cow::Borrowed(r#"{"properties":{"a":{"type":"keyword"}}}"#));
        let mut buf = vec![0u8; BUF];
        descriptors(store)
            .create(&record, 1, &mut buf)
            .expect("created");
    }

    fn stored(store: &Arc<Emulator>) -> (String, i64) {
        let mut buf = vec![0u8; BUF];
        let record = descriptors(store)
            .get("logs", &mut buf)
            .expect("read")
            .expect("there");
        (
            record.mapping.as_deref().unwrap_or("").to_owned(),
            record.mapping_version,
        )
    }

    #[test]
    fn a_swap_lost_to_another_update_reads_again_and_keeps_both_fields() {
        let store = store();
        create(&store);
        let svc = service(&store);
        let rival = service(&store);
        let before = stored(&store).1;
        let lost = std::cell::Cell::new(false);
        svc.update_mapping(
            "logs",
            br#"{"properties":{"b":{"type":"integer"}}}"#,
            &|_| {
                if !lost.replace(true) {
                    rival
                        .update_mapping(
                            "logs",
                            br#"{"properties":{"c":{"type":"integer"}}}"#,
                            &|_| {},
                        )
                        .expect("the rival wins the first swap");
                }
            },
        )
        .expect("the second attempt wins");
        let (mapping, version) = stored(&store);
        for field in ["\"a\"", "\"b\"", "\"c\""] {
            assert!(mapping.contains(field), "{field} in {mapping}");
        }
        assert_eq!(version, before + 2, "one move for each update");
    }

    #[test]
    fn four_lost_swaps_are_a_conflict() {
        let store = store();
        create(&store);
        let svc = service(&store);
        let rival = service(&store);
        let n = std::cell::Cell::new(0_u32);
        let err = svc
            .update_mapping(
                "logs",
                br#"{"properties":{"b":{"type":"integer"}}}"#,
                &|attempt| {
                    n.set(n.get() + 1);
                    let body = format!(r#"{{"properties":{{"r{attempt}":{{"type":"integer"}}}}}}"#);
                    rival
                        .update_mapping("logs", body.as_bytes(), &|_| {})
                        .expect("the rival wins");
                },
            )
            .expect_err("every swap is lost");
        assert_eq!(n.get(), ATTEMPTS, "each attempt was tried");
        assert!(format!("{err:?}").contains("409"), "{err:?}");
        assert!(!stored(&store).0.contains("\"b\""), "{}", stored(&store).0);
    }
}
