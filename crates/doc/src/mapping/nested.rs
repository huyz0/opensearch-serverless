//! The nested objects of a mapping.

use super::{FieldType, Mapper, Mapping, MappingError, ParseOptions};

impl Mapping {
    /// A `nested` object is accepted only by a caller that indexes it, and
    /// holds no vector field, which this writer would not index in a child.
    pub(super) fn check_nested(&self, options: ParseOptions) -> Result<(), MappingError> {
        let nested = self.nested_paths();
        if let Some(name) = nested.first().filter(|_| !options.nested) {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::unsupported(&format!(
                "field type [nested] on field [{name}]"
            )));
        }
        for (name, leaf) in self.leaves() {
            if leaf.ty() == FieldType::Knn
                && nested.iter().any(|n| {
                    name.strip_prefix(n.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
                })
            {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::unsupported(&format!(
                    "field type [knn_vector] on field [{name}] under a nested object"
                )));
            }
        }
        Ok(())
    }

    /// The full names of the nested objects, in name order.
    pub fn nested_paths(&self) -> Vec<String> {
        // ALLOC: one name a nested object, at most the nested fields limit once checked.
        let mut out = Vec::new();
        self.root.walk("", &mut |name, mapper| {
            if matches!(mapper, Mapper::Object(o) if o.nested) {
                // ALLOC: as above.
                out.push(name.to_owned());
            }
        });
        out
    }

    /// Whether `path` names a nested object.
    pub fn is_nested(&self, path: &str) -> bool {
        let mut obj = &self.root;
        for part in path.split('.') {
            match obj.child(part) {
                Some(Mapper::Object(o)) => obj = o,
                _ => return false,
            }
        }
        !path.is_empty() && obj.nested
    }

    /// Whether the mapping has a nested object.
    pub fn has_nested(&self) -> bool {
        !self.nested_paths().is_empty()
    }
}
