//! `index.max_result_window`, read from the settings an index opened with.

use crate::shard_files::IndexMeta;

impl IndexMeta {
    /// `index.max_result_window`: the most hits a search of this index may
    /// ask for (`from + size`). The node's ceiling when the index sets none
    /// or sets one that is not a whole number from 1 to that ceiling.
    #[must_use]
    pub fn max_result_window(&self) -> usize {
        use crate::search::MAX_RESULT_WINDOW;
        match self.settings.get("index.max_result_window") {
            Some(oss_format::Setting::Value(text)) => text
                .trim()
                .parse::<usize>()
                .ok()
                .filter(|w| (1..=MAX_RESULT_WINDOW).contains(w))
                .unwrap_or(MAX_RESULT_WINDOW),
            _ => MAX_RESULT_WINDOW,
        }
    }
}
