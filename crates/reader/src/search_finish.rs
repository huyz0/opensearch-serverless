//! What a search does with its hits before a response starts.

use crate::aggs::pipeline::script::settle;
use crate::failure::{Failure, SearchError};
use crate::search::Hits;

impl Hits {
    /// Keeps the first `cap` hits when a cap is set, then runs the script
    /// pipelines once so that a script that fails is a 400 `script_exception`
    /// rather than a response cut short.
    ///
    /// # Errors
    ///
    /// The failure of a script that throws or returns the wrong type.
    pub fn finish(&mut self, cap: Option<usize>) -> Result<(), Box<SearchError>> {
        if let Some(cap) = cap {
            self.truncate(cap);
        }
        settle(&self.aggregations, &self.pipelines, self.typed_keys).map_err(|f| {
            // ALLOC: error path; one box and message.
            Box::new(SearchError::request(
                400,
                Failure::plain("script_exception", f.to_string()),
            ))
        })
    }
}
