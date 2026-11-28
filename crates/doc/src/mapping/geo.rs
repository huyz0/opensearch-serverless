//! The `geo_point` fields of a mapping.

use super::{FieldType, Mapping, MappingError, ParseOptions};

impl Mapping {
    /// A `geo_point` field is accepted only by a caller that indexes it.
    pub(super) fn check_geo(&self, options: ParseOptions) -> Result<(), MappingError> {
        if options.geo {
            return Ok(());
        }
        for (name, leaf) in self.leaves() {
            if leaf.ty() == FieldType::GeoPoint {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::unsupported(&format!(
                    "field type [geo_point] on field [{name}]"
                )));
            }
        }
        Ok(())
    }
}
