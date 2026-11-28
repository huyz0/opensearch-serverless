//! What the SQL endpoint asks of a search service: an index's mapping and a
//! search by body over one or several names.

use oss_blob::BlobStore;
use oss_reader::dsl::parse_chunks;
use oss_rest::RestError;
use serde_json::Value;

use super::{Answer, SearchService, uri_query};
use crate::sql_route::{SqlBackend, SqlFailure};

fn failure(a: Answer) -> SqlFailure {
    match a {
        Answer::Rest(e) => SqlFailure::Rest(e),
        Answer::Search(e) => SqlFailure::Body(e.status, e.to_body()),
    }
}

impl<S: BlobStore + 'static> SqlBackend for SearchService<S> {
    fn mapping(&self, name: &str) -> Result<Value, SqlFailure> {
        let indices = self.expand_names([name]).map_err(SqlFailure::Rest)?;
        let Some(first) = indices.first() else {
            return Err(SqlFailure::Rest(RestError::illegal_argument(
                "no index named",
            )));
        };
        let held = self.open_shard(first, 0, false).map_err(failure)?;
        let json = held.shard.meta.mapping.json();
        if json.is_empty() {
            return Ok(serde_json::json!({}));
        }
        let v: Value = serde_json::from_str(json)
            .map_err(|e| SqlFailure::Rest(RestError::illegal_argument(e.to_string())))?;
        // A stored mapping may be wrapped under its type name.
        Ok(match v.get("properties") {
            Some(_) => v,
            None => v
                .as_object()
                .and_then(|o| o.values().find(|x| x.get("properties").is_some()).cloned())
                .unwrap_or(v),
        })
    }

    fn search(&self, names: &[String], body: &Value) -> Result<Vec<u8>, SqlFailure> {
        self.run_search(names, body, None)
    }

    fn search_until(
        &self,
        names: &[String],
        body: &Value,
        stop: &(dyn Fn() -> bool + Sync),
    ) -> Result<Vec<u8>, SqlFailure> {
        self.run_search(names, body, Some(stop))
    }
}

impl<S: BlobStore + 'static> SearchService<S> {
    fn run_search(
        &self,
        names: &[String],
        body: &Value,
        stop: Option<super::Stop<'_>>,
    ) -> Result<Vec<u8>, SqlFailure> {
        let bytes = body.to_string().into_bytes();
        let mut source =
            parse_chunks([bytes.as_slice()]).map_err(|e| failure(super::bad_request(e)))?;
        uri_query::stamp(&mut source);
        let indices = self
            .expand_names(names.iter().map(String::as_str))
            .map_err(SqlFailure::Rest)?;
        let answer = match indices.as_slice() {
            [one] => self.search_bytes_routed(one, &source, None, stop),
            many => self.search_bytes_many(many, &source, None, stop),
        };
        answer.map_err(failure)
    }
}
