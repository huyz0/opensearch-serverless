//! The engine over a real shard and its publisher.

use lucene_store::Directory;
use oss_format::WalRecord;

use super::Engine;
use crate::index::Shard;
use crate::publish::{PublishError, Published, ShardPublisher};

/// A [`Shard`], its directory and its [`ShardPublisher`], as one
/// [`Engine`].
pub struct ShardEngine<'a, 'd> {
    shard: &'a mut Shard<'d>,
    dir: &'d dyn Directory,
    publisher: &'a mut ShardPublisher,
    writer: &'a str,
    now_millis: u64,
    committed_through: i64,
}

impl<'a, 'd> ShardEngine<'a, 'd> {
    /// The engine of `shard`, as it was opened: its sequence numbers up to
    /// now are the commit's. It publishes through `publisher` as `writer`,
    /// `now_millis` naming a blob whose plain name is taken.
    #[must_use]
    pub fn new(
        shard: &'a mut Shard<'d>,
        dir: &'d dyn Directory,
        publisher: &'a mut ShardPublisher,
        writer: &'a str,
        now_millis: u64,
    ) -> Self {
        let committed_through = shard.max_seq_no();
        Self {
            shard,
            dir,
            publisher,
            writer,
            now_millis,
            committed_through,
        }
    }
}

impl Engine for ShardEngine<'_, '_> {
    fn committed_through(&self) -> i64 {
        self.committed_through
    }

    fn replay(&mut self, record: &WalRecord<'_>) -> Result<bool, String> {
        // ALLOC: one message, on a refused record.
        self.shard.replay(record).map_err(|e| {
            if record.has_sequence_identity() {
                // ALLOC: one message, on a refused record.
                format!("seq_no {}: {e:?}", record.seq_no())
            } else {
                // ALLOC: one message, on a refused record.
                format!("{e:?}")
            }
        })
    }

    fn buffered_bytes(&self) -> usize {
        self.shard.indexing_buffer_bytes()
    }

    fn flush(&mut self) -> Result<(), String> {
        self.shard.refresh()
    }

    fn publish(&mut self, term: u64) -> Result<Published, PublishError> {
        self.shard.refresh().map_err(PublishError::Directory)?;
        let files = self
            .shard
            .committed_files()
            .map_err(PublishError::Directory)?;
        self.publisher.publish(
            self.dir,
            &files,
            term,
            self.writer,
            // The log position the commit reaches is not known here.
            -1,
            self.now_millis,
        )
    }
}
