//! The descriptor store as the SQL endpoint's name listing.

use oss_meta::{DescriptorStore, MetaError};
use oss_rest::RestError;

use super::{NameLister, SqlFailure};

/// The names under a prefix, read from the descriptors: the listing `_cat/indices` makes.
pub struct DescriptorLister {
    descriptors: DescriptorStore,
}

impl DescriptorLister {
    /// A lister over `descriptors`.
    #[must_use]
    pub fn new(descriptors: DescriptorStore) -> Self {
        Self { descriptors }
    }
}

impl NameLister for DescriptorLister {
    fn names_with_prefix(&self, prefix: &str, cap: usize) -> Result<Vec<String>, SqlFailure> {
        self.descriptors
            .names_with_prefix(prefix, cap)
            .map_err(|e| match e {
                MetaError::TooManyMatches { .. } => {
                    SqlFailure::Rest(RestError::illegal_argument(e.to_string()))
                }
                other => SqlFailure::Rest(RestError::illegal_argument(format!(
                    "the index names could not be listed: {other}"
                ))),
            })
    }
}
