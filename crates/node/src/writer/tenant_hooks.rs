//! What a node's tenant levels hang on a writer role: who is told the
//! bucket group of an index when a shard of it is opened, and how far a
//! shard's publish age is stretched.

use std::sync::Arc;

use super::WriterRole;

/// Who is told an index's bucket group: index name and group.
pub type GroupSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

impl WriterRole {
    /// Installs who is told the bucket group of an index when a shard of it
    /// is opened. False when one is installed already.
    pub fn set_group_sink(&self, sink: GroupSink) -> bool {
        self.groups.set(sink).is_ok()
    }

    /// Tells the group sink that `index` bills to `group`.
    pub(crate) fn note_group(&self, index: &str, group: &str) {
        if let Some(sink) = self.groups.get() {
            sink(index, group);
        }
    }

    /// Installs how many times longer the writes of an index may wait to be
    /// published: the index's name in, the multiplier (1 for none) out.
    /// False when one is installed already, or this role does not publish.
    pub fn set_publish_stretch(&self, stretch: Arc<dyn Fn(&str) -> u64 + Send + Sync>) -> bool {
        let Some(publishing) = &self.publishing else {
            return false;
        };
        let names = Arc::clone(&self.names);
        publishing.set_stretch(Arc::new(move |uuid| {
            names.name_of(uuid).map_or(1, |index| stretch(&index))
        }))
    }
}
