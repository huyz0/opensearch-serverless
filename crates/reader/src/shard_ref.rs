//! The shard a query runs on, as a random score salts its seed with it.

/// The shard a query runs on.
#[derive(Clone, Copy, Debug)]
pub struct ShardRef<'a> {
    /// The index's name.
    pub index: &'a str,
    /// The index's uuid.
    pub uuid: &'a str,
    /// The shard's number.
    pub shard: u32,
}

impl<'a> ShardRef<'a> {
    /// `shard`, of index `index`.
    pub fn of<S: oss_blob::BlobStore + 'static>(
        shard: &'a crate::OpenShard<S>,
        index: &'a str,
    ) -> Self {
        Self {
            index,
            uuid: &shard.meta.uuid,
            shard: shard.shard,
        }
    }

    /// `(index.getName().hashCode() << 10) | shardId`: what a random score
    /// salts its seed with.
    pub fn salt(&self) -> i32 {
        let hash = crate::dsl::queries::function_score_random::string_hash(self.index);
        // A shard number is below 2^31 in any index this node serves.
        hash.wrapping_shl(10) | i32::try_from(self.shard).unwrap_or(0)
    }
}
