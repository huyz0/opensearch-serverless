//! Who is told the bucket group of an index when a shard of it is opened.

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
}
