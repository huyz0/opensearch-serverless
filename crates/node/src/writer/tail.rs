//! What the writer role tells the tail overlay as shards are written,
//! published and let go.

use oss_format::WalRecord;

/// Takes the events the tail overlay keeps and pushes records for. Called
/// outside any shard lock except `acked`, which runs after the log has the
/// records.
pub trait TailSink: Send + Sync {
    /// `records` of the shard were acknowledged.
    fn acked(&self, index: &str, uuid: &str, shard: u32, records: &[WalRecord<'static>]);
    /// A commit holding every operation through `checkpoint` was published.
    fn published(&self, uuid: &str, shard: u32, checkpoint: i64);
    /// The node no longer writes the shard.
    fn released(&self, uuid: &str, shard: u32);
}
