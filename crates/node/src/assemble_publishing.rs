//! The publish pass a node's configuration asks for.

use std::sync::Arc;

use oss_blob::BlobStore;

use crate::assemble::Config;
use crate::writer::Publishing;

impl Config {
    /// Publishing into `store` by this configuration's lazy policy, staging
    /// its routine commits when it names a checkpoint age.
    pub(crate) fn publishing(&self, store: Arc<dyn BlobStore>) -> Publishing {
        Publishing::from_parts(
            store,
            &self.base,
            &self.node_id,
            [
                self.publish_age_millis,
                self.publish_max_operations,
                self.publish_tick_millis,
            ],
            self.commit_checkpoint_millis,
        )
    }
}
