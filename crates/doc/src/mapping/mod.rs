//! An index's mapping: read from a request or a descriptor, merged with an
//! update, and written back in the form `MapperService` keeps.
//!
//! Ported from `MapperService.merge`, `DocumentMapperParser.parse`,
//! `RootObjectMapper` and the field mappers in `field` (server,
//! OpenSearch 3.10), for the field types this writer indexes. The written
//! form is `DocumentMapper.mappingSource()` without its `_doc` wrapper, as
//! the shell stores it after an update and as `GET /<index>/_mapping`
//! shows it: `dynamic`, `enabled`, metadata fields, the root's options,
//! then `properties`, children sorted by name, each parameter written only
//! when it differs from its default.
//!
//! The shell re-serializes a merged mapping through a `HashMap`, so the
//! mappings it stores keep their members in hash order; two mappings are
//! the same when they parse to the same [`Mapping`], whatever their order.
//! A mapping this writer cannot index (a type or parameter it does not
//! support) is refused with [`MappingError::unsupported`].

mod date_text;
mod dates;
mod error;
mod field;
pub mod json;
mod numbers;
mod object;
mod params;
mod types;

pub use dates::{Format, Formats};
pub use error::{MappingError, UNSUPPORTED};
pub use field::Leaf;
pub use numbers::{Num, NumType, java_parse_double};
pub use object::{Dynamic, Mapper, Object};
pub use params::Value;
pub use types::{DEFAULT_DATE_FORMAT, FieldType, POSITION_INCREMENT_GAP_USE_ANALYZER};

use json::Json;
pub(crate) use object::join;
use object::remaining;
use params::{named_bool, parse_bool};

/// `index.mapping.total_fields.limit`'s default.
pub const TOTAL_FIELDS_LIMIT: usize = 1000;

/// `index.mapping.depth.limit`'s default.
pub const DEPTH_LIMIT: usize = 20;

/// A parsed mapping.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    root: Object,
    /// `_routing.required`, when the mapping sets it.
    routing_required: Option<bool>,
    /// `date_detection`, when set.
    date_detection: Option<bool>,
    /// `numeric_detection`, when set.
    numeric_detection: Option<bool>,
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            root: Object::new(),
            routing_required: None,
            date_detection: None,
            numeric_detection: None,
        }
    }
}

impl Mapping {
    /// Reads a mapping as a create-index or put-mapping request gives it,
    /// or a descriptor stores it: `{"properties": ...}`, or the same
    /// wrapped in `{"_doc": ...}`.
    ///
    /// # Errors
    ///
    /// `Failed to parse mapping [_doc]: ...` caused by the Java's message,
    /// or [`MappingError::unsupported`].
    pub fn parse(text: &[u8]) -> Result<Self, MappingError> {
        let mapping = Self::parse_unwrapped(text).map_err(MappingError::failed_to_parse)?;
        // `MapperService` checks the limits after the parse, unwrapped.
        mapping.check_limits()?;
        Ok(mapping)
    }

    fn parse_unwrapped(text: &[u8]) -> Result<Self, MappingError> {
        let json = json::parse(text)?;
        let Json::Obj(mut members) = json else {
            return Err(MappingError::mapper_parsing("the mapping is not an object"));
        };
        if let [(key, Json::Obj(inner))] = members.as_slice()
            && key == "_doc"
        {
            members = inner.clone();
        }
        let mut mapping = Mapping::default();
        // ALLOC: the root's keys the object parser leaves; parse path.
        let mut rest = Vec::new();
        mapping.root.parse_into("", &members, &mut rest)?;
        // ALLOC: the keys left after the root's own; parse path.
        let mut unknown = Vec::new();
        for (key, value) in rest {
            match key.as_str() {
                "date_detection" => mapping.date_detection = Some(root_bool(&key, &value)?),
                "numeric_detection" => mapping.numeric_detection = Some(root_bool(&key, &value)?),
                "_routing" => mapping.routing_required = Some(routing(&value)?),
                "dynamic_date_formats"
                | "date_formats"
                | "dynamic_templates"
                | "dynamic_properties"
                | "_source"
                | "_meta"
                | "_field_names"
                | "_size"
                | "_data_stream_timestamp" => {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::unsupported(&format!("root option [{key}]")));
                }
                _ => unknown.push((key, value)),
            }
        }
        if !unknown.is_empty() {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::mapper_parsing(&format!(
                "Root mapping definition has unsupported parameters: {}",
                remaining(&unknown)
            )));
        }
        Ok(mapping)
    }

    /// Reads the mapping a descriptor stores, which may be in any member
    /// order (see the module docs). A missing mapping is the empty one.
    ///
    /// # Errors
    ///
    /// As [`Mapping::parse`].
    pub fn from_descriptor(text: Option<&str>) -> Result<Self, MappingError> {
        match text {
            None => Ok(Mapping::default()),
            Some(t) => Self::parse(t.as_bytes()),
        }
    }

    /// The root object.
    pub fn root(&self) -> &Object {
        &self.root
    }

    /// The root object, to grow it with dynamic fields.
    pub fn root_mut(&mut self) -> &mut Object {
        &mut self.root
    }

    /// `date_detection`, true by default.
    pub fn date_detection(&self) -> bool {
        self.date_detection.unwrap_or(true)
    }

    /// `numeric_detection`, false by default.
    pub fn numeric_detection(&self) -> bool {
        self.numeric_detection.unwrap_or(false)
    }

    /// `_routing.required`, false by default.
    pub fn routing_required(&self) -> bool {
        self.routing_required.unwrap_or(false)
    }

    /// Merges an update into this mapping as `MapperService.merge` does for
    /// `PUT /<index>/_mapping`, and as dynamic mapping grows a mapping.
    ///
    /// # Errors
    ///
    /// The Java's messages for conflicting fields and exceeded limits.
    pub fn merge(&self, update: &Mapping) -> Result<Mapping, MappingError> {
        let root = self.root.merge(&update.root, "")?;
        let routing_required = match (self.routing_required, update.routing_required) {
            (current, None) => current,
            (current, Some(next)) if current.unwrap_or(false) == next => Some(next),
            (current, Some(next)) => {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::illegal_argument(&format!(
                    "Mapper for [_routing] conflicts with existing mapper:\n\tCannot update parameter [required] from [{}] to [{next}]",
                    current.unwrap_or(false)
                )));
            }
        };
        let merged = Mapping {
            root,
            routing_required,
            date_detection: update.date_detection.or(self.date_detection),
            numeric_detection: update.numeric_detection.or(self.numeric_detection),
        };
        merged.check_limits()?;
        Ok(merged)
    }

    /// The mapping as `mappingSource()` writes it, without the `_doc`
    /// wrapper.
    pub fn to_json(&self) -> String {
        // ALLOC: the mapping's text, bounded by TOTAL_FIELDS_LIMIT fields.
        let mut out = String::new();
        let extra = |out: &mut String, first: &mut bool| {
            let field = |out: &mut String, first: &mut bool, name: &str, value: bool| {
                if !*first {
                    out.push(',');
                }
                *first = false;
                out.push('"');
                out.push_str(name);
                out.push_str("\":");
                out.push_str(if value { "true" } else { "false" });
            };
            if let Some(required) = self.routing_required.filter(|r| *r) {
                if !*first {
                    out.push(',');
                }
                *first = false;
                out.push_str("\"_routing\":{\"required\":");
                out.push_str(if required { "true" } else { "false" });
                out.push('}');
            }
            if let Some(d) = self.date_detection {
                field(out, first, "date_detection", d);
            }
            if let Some(n) = self.numeric_detection {
                field(out, first, "numeric_detection", n);
            }
        };
        self.root.write_body(&mut out, Some(&extra));
        out
    }

    /// Every leaf field, multi-fields included, by full name.
    pub fn leaves(&self) -> Vec<(String, &Leaf)> {
        // ALLOC: one entry a field, at most TOTAL_FIELDS_LIMIT.
        let mut out = Vec::new();
        self.root.walk("", &mut |name, mapper| {
            if let Mapper::Leaf(leaf) = mapper {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                out.push((name.to_owned(), leaf));
                for (sub, sub_leaf) in leaf.multi_fields() {
                    out.push((join(name, sub), sub_leaf));
                }
            }
        });
        out
    }

    /// `MappingLookup.checkLimits`: fields and objects at most
    /// [`TOTAL_FIELDS_LIMIT`], objects at most [`DEPTH_LIMIT`] deep.
    /// How many fields the total-fields limit counts: every object and
    /// field, and every multi-field.
    pub fn field_count(&self) -> usize {
        let mut count = 0usize;
        self.root.walk("", &mut |_, mapper| {
            count = count.saturating_add(1);
            if let Mapper::Leaf(leaf) = mapper {
                count = count.saturating_add(leaf.multi_fields().len());
            }
        });
        count
    }

    fn check_limits(&self) -> Result<(), MappingError> {
        let mut count = 0usize;
        let mut too_deep: Option<String> = None;
        self.root.walk("", &mut |name, mapper| {
            count = count.saturating_add(1);
            match mapper {
                Mapper::Leaf(leaf) => count = count.saturating_add(leaf.multi_fields().len()),
                Mapper::Object(_) => {
                    let depth = name.matches('.').count().saturating_add(2);
                    if depth > DEPTH_LIMIT && too_deep.is_none() {
                        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                        too_deep = Some(name.to_owned());
                    }
                }
            }
        });
        if count > TOTAL_FIELDS_LIMIT {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::illegal_argument(&format!(
                "Limit of total fields [{TOTAL_FIELDS_LIMIT}] has been exceeded"
            )));
        }
        if let Some(path) = too_deep {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::illegal_argument(&format!(
                "Limit of mapping depth [{DEPTH_LIMIT}] has been exceeded due to object field [{path}]"
            )));
        }
        Ok(())
    }
}

fn root_bool(key: &str, value: &Json) -> Result<bool, MappingError> {
    named_bool(key, value)
}

/// `_routing`: `{"required": <bool>}`.
fn routing(value: &Json) -> Result<bool, MappingError> {
    let Json::Obj(members) = value else {
        return Err(MappingError::illegal_argument(
            "[_parent] must be an object containing [type]",
        ));
    };
    let mut required = false;
    // ALLOC: error path only.
    let mut rest = Vec::new();
    for (k, v) in members {
        match k.as_str() {
            "required" => required = parse_bool(v)?,
            "type" => {}
            _ => rest.push((k.clone(), v.clone())),
        }
    }
    if !rest.is_empty() {
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::mapper_parsing(&format!(
            "unknown parameter [{}] on mapper [_routing] of type [null]",
            rest.first().map(|(k, _)| k.as_str()).unwrap_or_default()
        )));
    }
    Ok(required)
}
