//! A shard's refresh and the files of its last commit.

use super::{
    GC_DELETES_MILLIS, HISTORY_UUID_KEY, LOCAL_CHECKPOINT_KEY, MAX_SEQ_NO_KEY, Shard,
    TRANSLOG_UUID_KEY, message,
};

impl Shard<'_> {
    /// Commits what the writer holds and looks up versions in the new
    /// segments, keeping only deletes in the version map.
    ///
    /// # Errors
    ///
    /// The commit or the new reader failing.
    pub fn refresh(&mut self) -> Result<(), String> {
        // A primary's writes complete in order, so the local checkpoint is
        // the highest sequence number given.
        let mut data: Vec<(String, String)> = self
            .writer
            .live_commit_data()
            .iter()
            .filter(|(k, _)| k != MAX_SEQ_NO_KEY && k != LOCAL_CHECKPOINT_KEY)
            // ALLOC: the commit's user data, a few entries, once a refresh.
            .cloned()
            .collect();
        // ALLOC: one number's text a refresh.
        let seq = self.max_seq_no.to_string();
        // A shard made from nothing carries the ids `Store.createEmpty` gives,
        // so a Java writer taking it over finds the translog UUID its engine
        // requires; a commit that has them keeps them.
        for key in [TRANSLOG_UUID_KEY, HISTORY_UUID_KEY] {
            if !data.iter().any(|(k, _)| k == key) {
                let id = oss_meta::new_id().map_err(|e| message(&e))?;
                // ALLOC: a key's text, twice on a shard's first commit.
                data.push((key.to_owned(), id));
            }
        }
        // ALLOC: two entries of commit user data a refresh.
        data.push((LOCAL_CHECKPOINT_KEY.to_owned(), seq.clone()));
        data.push((MAX_SEQ_NO_KEY.to_owned(), seq));
        self.writer.set_live_commit_data(data);
        self.writer.commit().map_err(|e| message(&e))?;
        // Merging after the commit, under the node's guard, then a second
        // commit that names what the merge made.
        // A merge that fails is skipped: the commit above is durable, and
        // the next refresh's commit names whatever the writer holds then.
        match self.merge_small_segments() {
            Ok(true) => {
                if self.writer.commit().is_err() {
                    self.merge_failures = self.merge_failures.saturating_add(1);
                }
            }
            Ok(false) => {}
            Err(_) => self.merge_failures = self.merge_failures.saturating_add(1),
        }
        self.open_reader()?;
        self.versions.refreshed();
        self.versions
            .prune_deletes((self.clock)(), GC_DELETES_MILLIS);
        Ok(())
    }

    /// How many merges, or commits that named a merge, failed and were skipped.
    #[must_use]
    pub fn merge_failures(&self) -> u64 {
        self.merge_failures
    }

    /// The files of the last commit, `segments_N` included: what a publish
    /// of the commit `refresh` made uploads.
    ///
    /// # Errors
    ///
    /// A segment's files not listing.
    pub fn committed_files(&self) -> Result<Vec<String>, String> {
        lucene_index::index_file_deleter::files_of_infos(
            self.dir,
            self.writer.segment_infos(),
            true,
        )
        .map_err(|e| message(&e))
    }
}
