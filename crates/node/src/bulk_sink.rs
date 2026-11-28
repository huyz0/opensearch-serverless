//! `index.docs` for plugins: documents indexed through the same service as
//! `_bulk`, so a plugin's writes take the writer path, its memory budget and
//! its refusals, and nothing else writes.

use oss_ext::doc_sink::{Doc, DocResult, DocSink};
use oss_ext::host::HostError;
use oss_writer::bulk::{Defaults, doc_status};

use crate::bulk_route::BulkService;

/// A `_bulk` body for `docs`: each source on one line after its action line.
fn body(index: &str, docs: &[Doc]) -> Result<Vec<u8>, HostError> {
    // ALLOC: one call's documents, at most the host op's ceilings.
    let mut out = Vec::new();
    for doc in docs {
        let source: serde_json::Value =
            serde_json::from_slice(&doc.source).map_err(|_| HostError::Invalid)?;
        if !source.is_object() {
            return Err(HostError::Invalid);
        }
        let mut action = serde_json::Map::new();
        action.insert("_index".into(), index.into());
        if let Some(id) = &doc.id {
            action.insert("_id".into(), id.as_str().into());
        }
        out.extend_from_slice(
            serde_json::json!({ "index": action })
                .to_string()
                .as_bytes(),
        );
        out.push(b'\n');
        out.extend_from_slice(source.to_string().as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

impl DocSink for BulkService {
    fn index(&self, index: &str, docs: &[Doc]) -> Result<Vec<DocResult>, HostError> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let defaults = Defaults {
            index: Some(index.to_owned()),
            routing: None,
            pipeline: None,
            require_alias: false,
        };
        let items = self
            .apply(&defaults, &body(index, docs)?)
            .map_err(|e| match e.status() {
                400 => HostError::Invalid,
                _ => HostError::Failed,
            })?;
        Ok(items
            .iter()
            .map(|item| DocResult {
                id: item.id.clone(),
                status: doc_status(item),
            })
            .collect())
    }
}
