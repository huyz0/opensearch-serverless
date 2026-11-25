//! Holding a write's new fields in the index's descriptor.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::WalRecord;
use oss_meta::DescriptorStore;

use oss_writer::bulk::IndexInfo;
use oss_writer::mapping_growth::MappingGrowth;

use super::WriterRole;
use super::slot::ShardSlot;

impl WriterRole {
    #[cfg(feature = "reader")]
    /// Grows the descriptors of this role's indexes with a descriptor store over `store`;
    /// false when a growth is set already.
    ///
    /// # Errors
    ///
    /// The descriptor store refusing the config's base.
    pub fn set_mapping_growth(
        &self,
        store: &Arc<dyn BlobStore>,
        config: &crate::assemble::Config,
    ) -> Result<bool, crate::assemble::AssembleError> {
        let descriptors = DescriptorStore::new(Arc::clone(store), &config.base, config.delete_mode)
            .map_err(|e| crate::assemble::AssembleError::Store(e.to_string()))?;
        let growth = MappingGrowth::new(Arc::new(descriptors));
        Ok(self.growth.set(growth).is_ok())
    }

    /// Stages `records` in the log once the descriptor holds the fields the
    /// group added, so a reader never finds a document whose field it cannot
    /// map.
    pub(super) fn commit_group(
        &self,
        slot: &mut ShardSlot,
        index: &IndexInfo,
        records: &[WalRecord<'_>],
    ) -> Result<(), String> {
        self.hold_growth(slot, index)?;
        self.committer
            .commit(slot.key(), slot.term(), records)
            .map(|_| ())
            .map_err(|e| format!("the write could not be made durable: {e}"))
    }

    /// Writes to the descriptor of `index` what the writes applied to `slot`
    /// added to its mapping; nothing when they added none, or when no growth
    /// is set.
    ///
    /// # Errors
    ///
    /// Why the descriptor could not hold it: the writes are then not logged.
    pub(super) fn hold_growth(
        &self,
        slot: &mut ShardSlot,
        index: &IndexInfo,
    ) -> Result<(), String> {
        let Some(addition) = slot.take_growth() else {
            return Ok(());
        };
        let Some(growth) = self.growth.get() else {
            return Ok(());
        };
        growth
            .grow(&index.name, &index.uuid, &addition)
            .map(|_| ())
            .map_err(|e| format!("the mapping update could not be made durable: {e}"))
    }
}
