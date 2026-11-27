//! `GET|POST /{index}/_field_caps?fields=`: which fields indices map.
//!
//! Ported from `RestFieldCapabilitiesAction` and `FieldCapabilitiesResponse`
//! (server/src/main/java/org/opensearch/action/fieldcaps/) at OpenSearch
//! 3.10.0 for concrete index names and the `fields` parameter. Each index is
//! opened as a search opens it and its mapping's leaf fields are read from
//! the open shard.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_doc::mapping::json::write_str;
use oss_doc::mapping::{FieldType, Mapping, Value};
use oss_rest::{Exception, JSON, Method, Request, Response, RestError, RouteError, Router};

use super::{Answer, SearchService, send};

/// The URL parameters `_field_caps` takes.
const FIELD_CAPS_PARAMS: &[&str] = &["fields", "ignore_unavailable"];

/// The routes of `_field_caps`: on named indices by GET and POST, and the
/// index-less forms, which answer the refusal.
pub(super) fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Get, Method::Post] {
        for path in ["/{index}/_field_caps", "/_field_caps"] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                FIELD_CAPS_PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle_field_caps(req, resp)
                },
            )?;
        }
    }
    Ok(router)
}

/// Field, then type, then each index that has it (its position) with what it says.
type Merged<'a> = BTreeMap<&'a str, BTreeMap<&'a str, Vec<(usize, &'a Cap)>>>;

/// What one index says of one field.
struct Cap {
    ty: &'static str,
    searchable: bool,
    aggregatable: bool,
}

/// `*` matches any run of characters, as the Java's `Regex.simpleMatch`.
fn glob(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((head, rest)) => {
            let Some(mut tail) = name.strip_prefix(head) else {
                return false;
            };
            let mut parts = rest.split('*').peekable();
            while let Some(part) = parts.next() {
                if parts.peek().is_none() {
                    return tail.ends_with(part);
                }
                match tail.find(part) {
                    Some(at) => tail = tail.get(at.saturating_add(part.len())..).unwrap_or(""),
                    None => return false,
                }
            }
            true
        }
    }
}

fn flag(leaf: &oss_doc::mapping::Leaf, name: &str) -> bool {
    matches!(
        leaf.get(name),
        Some(Value::Bool(true) | Value::Explicit(true, _))
    )
}

fn caps_of(mapping: &Mapping, patterns: &[&str]) -> BTreeMap<String, Cap> {
    let mut out = BTreeMap::new();
    for (name, leaf) in mapping.leaves() {
        if !patterns.iter().any(|p| glob(p, &name)) {
            continue;
        }
        let ty = leaf.ty();
        let aggregatable = match ty {
            FieldType::Text => flag(leaf, "fielddata"),
            _ => flag(leaf, "doc_values"),
        };
        // ALLOC: one entry a matching field, at most the mapping's field limit.
        out.insert(
            name,
            Cap {
                ty: ty.name(),
                searchable: flag(leaf, "index"),
                aggregatable,
            },
        );
    }
    out
}

fn refusal(reason: &str) -> RestError {
    RestError::illegal_argument(format!("{reason} is not supported by this node yet"))
}

/// The answer for `indices` and the capabilities each reported.
fn render(indices: &[String], per_index: &[BTreeMap<String, Cap>]) -> String {
    // field -> type -> (index position, cap)
    let mut merged: Merged<'_> = BTreeMap::new();
    for (at, caps) in per_index.iter().enumerate() {
        for (name, cap) in caps {
            merged
                .entry(name)
                .or_default()
                .entry(cap.ty)
                .or_default()
                .push((at, cap));
        }
    }
    let names = |picked: &mut dyn Iterator<Item = usize>| {
        let list: Vec<String> = picked
            .filter_map(|at| indices.get(at))
            .map(|n| {
                let mut q = String::new();
                write_str(&mut q, n);
                q
            })
            .collect();
        list.join(",")
    };
    // ALLOC: the response, bounded by fields times indices.
    let mut out = format!(
        "{{\"indices\":[{}],\"fields\":{{",
        names(&mut (0..indices.len()))
    );
    for (i, (field, types)) in merged.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(&mut out, field);
        out.push_str(":{");
        let several = types.len() > 1;
        for (j, (ty, entries)) in types.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            let searchable = entries.iter().all(|(_, c)| c.searchable);
            let aggregatable = entries.iter().all(|(_, c)| c.aggregatable);
            let _ = write!(
                out,
                "\"{ty}\":{{\"type\":\"{ty}\",\"metadata_field\":false,\"searchable\":{searchable},\"aggregatable\":{aggregatable}"
            );
            if several {
                let _ = write!(
                    out,
                    ",\"indices\":[{}]",
                    names(&mut entries.iter().map(|(at, _)| *at))
                );
            }
            if !searchable && entries.iter().any(|(_, c)| c.searchable) {
                let _ = write!(
                    out,
                    ",\"non_searchable_indices\":[{}]",
                    names(
                        &mut entries
                            .iter()
                            .filter(|(_, c)| !c.searchable)
                            .map(|(at, _)| *at)
                    )
                );
            }
            if !aggregatable && entries.iter().any(|(_, c)| c.aggregatable) {
                let _ = write!(
                    out,
                    ",\"non_aggregatable_indices\":[{}]",
                    names(
                        &mut entries
                            .iter()
                            .filter(|(_, c)| !c.aggregatable)
                            .map(|(at, _)| *at)
                    )
                );
            }
            out.push('}');
        }
        out.push('}');
    }
    out.push_str("}}");
    out
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Handles one `_field_caps`.
    pub fn handle_field_caps(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let Some(asked) = req.path_param("index") else {
            return Err(refusal("_field_caps without an index"));
        };
        let Some(fields) = req.param("fields") else {
            return Err(RestError::exception(
                400,
                Exception::new(
                    "action_request_validation_exception",
                    "Validation Failed: 1: no fields specified;",
                ),
            ));
        };
        let indices: Vec<String> = asked.split(',').map(str::to_owned).collect();
        if indices
            .iter()
            .any(|i| i.is_empty() || i == "_all" || i.contains('*'))
        {
            return Err(refusal(
                "a pattern, _all or an empty name as the index of _field_caps",
            ));
        }
        // An alias is its indexes; the answer names them.
        let mut indices = self.expand_names(indices.iter().map(String::as_str))?;
        if super::multi::ignore_unavailable(req)? {
            indices = self.skip_unavailable(indices)?;
        }
        let patterns: Vec<&str> = fields.split(',').filter(|p| !p.is_empty()).collect();
        let mut per_index = Vec::new();
        for index in &indices {
            let held = match self.open_shard(index, 0, false) {
                Ok(held) => held,
                Err(Answer::Rest(e)) => return Err(e),
                Err(Answer::Search(e)) => return send(resp, &e),
            };
            let json = held.shard.meta.mapping.json();
            let mapping = if json.is_empty() {
                Mapping::default()
            } else {
                // ALLOC: one mapping a call, bounded by the descriptor's mapping text.
                Mapping::from_descriptor(Some(json)).map_err(|e| {
                    RestError::exception(
                        500,
                        Exception::new("mapper_parsing_exception", e.to_string()),
                    )
                })?
            };
            per_index.push(caps_of(&mapping, &patterns));
        }
        let body = render(&indices, &per_index);
        resp.send(200, JSON, body.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(ty: &'static str) -> BTreeMap<String, Cap> {
        let mut caps = BTreeMap::new();
        caps.insert(
            "year".to_owned(),
            Cap {
                ty,
                searchable: true,
                aggregatable: true,
            },
        );
        caps
    }

    #[test]
    fn a_field_two_indices_type_differently_lists_each_type_after_a_comma() {
        let out = render(
            &["a".to_owned(), "b".to_owned()],
            &[cap("integer"), cap("long")],
        );
        assert_eq!(
            out,
            concat!(
                r#"{"indices":["a","b"],"fields":{"year":{"#,
                r#""integer":{"type":"integer","metadata_field":false,"searchable":true,"aggregatable":true,"indices":["a"]},"#,
                r#""long":{"type":"long","metadata_field":false,"searchable":true,"aggregatable":true,"indices":["b"]}}}}"#,
            )
        );
    }

    #[test]
    fn a_field_of_one_type_has_no_leading_comma() {
        let out = render(&["a".to_owned()], &[cap("long")]);
        assert_eq!(
            out,
            concat!(
                r#"{"indices":["a"],"fields":{"year":{"#,
                r#""long":{"type":"long","metadata_field":false,"searchable":true,"aggregatable":true}}}}"#,
            )
        );
    }
}
