//! Leaf fields: the types this writer indexes, their parameters, and how a
//! field is read from a mapping, written back and merged.
//!
//! Ported from `TextFieldMapper`, `KeywordFieldMapper`, `NumberFieldMapper`,
//! `DateFieldMapper` and `BooleanFieldMapper` (server, OpenSearch 3.10), each
//! through `ParametrizedFieldMapper`. Every parameter those mappers take is
//! read; one this writer does not index differently yet (a similarity, a
//! normalizer, term vectors) is refused as unsupported when it is set away
//! from its default, so such an index stays with a Java writer.

use std::fmt::Write as _;

use super::error::MappingError;
use super::json::{Json, java_cmp, write_str};
use super::numbers::NumType;
use super::params::{Default as D, Kind, Merge, Value};
pub use super::types::FieldType;
use super::types::{
    DEFAULT_DATE_FORMAT, DEPRECATED_PARAMS, OTHER_TYPES, POSITION_INCREMENT_GAP_USE_ANALYZER,
    TEXT_ORDER,
};

/// A leaf field: its type, every parameter's value and its multi-fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    ty: FieldType,
    values: Vec<Value>,
    /// Multi-fields by simple name, sorted.
    fields: Vec<(String, Leaf)>,
}

impl Leaf {
    /// A field of `ty` with every parameter at its default.
    pub fn new(ty: FieldType) -> Self {
        let defs = ty.defs();
        // ALLOC: one value a parameter, at most 17.
        let mut values: Vec<Value> = Vec::with_capacity(defs.len());
        for def in defs {
            let v = match def.default {
                D::Same(i) => values.get(i).cloned().unwrap_or(Value::Null),
                d => fixed_default(d, def.kind),
            };
            values.push(v);
        }
        Self {
            ty,
            values,
            // ALLOC: none until a multi-field is added.
            fields: Vec::new(),
        }
    }

    /// The field's type.
    pub fn ty(&self) -> FieldType {
        self.ty
    }

    /// A parameter's value, by name.
    pub fn get(&self, name: &str) -> Option<&Value> {
        let i = self.ty.defs().iter().position(|d| d.name == name)?;
        self.values.get(i)
    }

    /// Sets a parameter, by name; for building dynamic fields.
    pub fn set(&mut self, name: &str, value: Value) {
        if let Some(i) = self.ty.defs().iter().position(|d| d.name == name)
            && let Some(slot) = self.values.get_mut(i)
        {
            *slot = value;
        }
    }

    /// The multi-fields, by simple name, sorted.
    pub fn multi_fields(&self) -> &[(String, Leaf)] {
        &self.fields
    }

    /// Adds a multi-field; for building dynamic fields.
    pub fn add_multi_field(&mut self, name: &str, leaf: Leaf) {
        // ALLOC: one name; mapping-update path.
        self.fields.push((name.to_owned(), leaf));
        self.fields.sort_by(|a, b| java_cmp(&a.0, &b.0));
    }

    /// The default of parameter `i`, given the values of the others.
    fn default_of(&self, i: usize) -> Value {
        let defs = self.ty.defs();
        match defs.get(i).map(|d| d.default) {
            Some(D::Same(j)) => self.values.get(j).cloned().unwrap_or(Value::Null),
            Some(d) => fixed_default(d, defs.get(i).map_or(Kind::Str, |d| d.kind)),
            None => Value::Null,
        }
    }

    /// Whether parameter `i` differs from its default (`isConfigured`).
    fn configured(&self, i: usize) -> bool {
        self.values.get(i).is_some_and(|v| *v != self.default_of(i))
    }

    /// Reads a field of `ty` named `name` from its mapping object.
    ///
    /// # Errors
    ///
    /// OpenSearch's `MapperParsingException` and `IllegalArgumentException`
    /// messages, or [`MappingError::unsupported`].
    pub fn parse(
        name: &str,
        ty: FieldType,
        node: &[(String, Json)],
        multi: bool,
    ) -> Result<Self, MappingError> {
        let mut leaf = Leaf::new(ty);
        let defs = ty.defs();
        // Which parameters the mapping set: at most 17, one bit each.
        let mut set: u32 = 0;
        for (key, value) in node {
            if key == "type" {
                continue;
            }
            if key == "fields" {
                leaf.parse_multi_fields(name, value, multi)?;
                continue;
            }
            if key == "copy_to" {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::unsupported(&format!(
                    "[copy_to] on field [{name}]"
                )));
            }
            let Some(i) = defs.iter().position(|d| d.name == key) else {
                if DEPRECATED_PARAMS.contains(&key.as_str()) {
                    continue;
                }
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "unknown parameter [{key}] on mapper [{name}] of type [{}]",
                    ty.name()
                )));
            };
            let Some(def) = defs.get(i) else { continue };
            let parsed = super::read::parse_value(name, ty, def, value)?;
            if let Some(slot) = leaf.values.get_mut(i) {
                *slot = parsed;
            }
            set |= 1u32
                .checked_shl(u32::try_from(i).unwrap_or(31))
                .unwrap_or(0);
        }
        leaf.follow_unset(set);
        leaf.validate(name)?;
        if ty == FieldType::Knn {
            leaf.resolve_knn(name)?;
        }
        Ok(leaf)
    }

    /// The checks the k-NN plugin's `TypeParser` makes after reading the
    /// parameters, and the method it keeps: see [`super::knn::resolve`].
    fn resolve_knn(&mut self, name: &str) -> Result<(), MappingError> {
        let dimension = match self.get("dimension") {
            Some(Value::Int(d)) => *d,
            _ => super::types::KNN_NO_DIMENSION,
        };
        let method = match self.get("method") {
            Some(Value::Method(m)) => Some(super::knn::KnnMethod::clone(m)),
            _ => None,
        };
        let top = self
            .get("space_type")
            .and_then(Value::text)
            .unwrap_or("undefined");
        let resolved = super::knn::resolve(name, dimension, method.as_ref(), top)?;
        if let (Some(m), Some(i)) = (
            resolved,
            self.ty.defs().iter().position(|d| d.name == "method"),
        ) && let Some(slot) = self.values.get_mut(i)
        {
            *slot = Value::Method(Box::new(m));
        }
        Ok(())
    }

    /// A parameter defaulting to another's (`search_analyzer` to
    /// `analyzer`) takes that one's value when the mapping left it unset.
    fn follow_unset(&mut self, set: u32) {
        for (i, def) in self.ty.defs().iter().enumerate() {
            let was_set = set
                & 1u32
                    .checked_shl(u32::try_from(i).unwrap_or(31))
                    .unwrap_or(0)
                != 0;
            if let (D::Same(j), false) = (def.default, was_set)
                && let Some(v) = self.values.get(j).cloned()
                && let Some(slot) = self.values.get_mut(i)
            {
                *slot = v;
            }
        }
    }

    fn parse_multi_fields(
        &mut self,
        name: &str,
        node: &Json,
        multi: bool,
    ) -> Result<(), MappingError> {
        let members = match node {
            Json::Obj(m) => m.as_slice(),
            Json::Arr(a) if a.is_empty() => &[],
            other => {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "expected map for property [fields] on field [{}] or [fields] but got a class {}.{}",
                    other.java_string(),
                    super::params::java_package(other),
                    super::params::java_class(other)
                )));
            }
        };
        if multi && !members.is_empty() {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::unsupported(&format!(
                "multi-fields within the multi-field [{name}]"
            )));
        }
        for (sub, value) in members {
            if sub.contains('.') {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "Field name [{sub}] which is a multi field of [{name}] cannot contain '.'"
                )));
            }
            let Json::Obj(sub_node) = value else {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "illegal field [{sub}], only fields can be specified inside fields"
                )));
            };
            let ty_name = match value.get("type") {
                Some(t) => t.java_string(),
                None => {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::mapper_parsing(&format!(
                        "no type specified for property [{sub}]"
                    )));
                }
            };
            if matches!(ty_name.as_str(), "object" | "nested" | "alias") {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "Type [{ty_name}] cannot be used in multi field"
                )));
            }
            let ty = resolve_type(&ty_name, sub, "no handler for type")?;
            let leaf = Leaf::parse(sub, ty, sub_node, true)?;
            self.fields.retain(|(n, _)| n != sub);
            // ALLOC: one name a multi-field; mapping parse path.
            self.fields.push((sub.clone(), leaf));
        }
        self.fields.sort_by(|a, b| java_cmp(&a.0, &b.0));
        Ok(())
    }

    /// The checks the Java's `build` makes, and the parameters this writer
    /// does not index yet.
    fn validate(&self, name: &str) -> Result<(), MappingError> {
        if name.is_empty() {
            return Err(MappingError::illegal_argument(
                "name cannot be empty string",
            ));
        }
        for (i, def) in self.ty.defs().iter().enumerate() {
            if def.only_default && self.configured(i) {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::unsupported(&format!(
                    "[{}] on field [{name}]",
                    def.name
                )));
            }
        }
        if self.ty == FieldType::Number(NumType::ScaledFloat)
            && !matches!(self.get("scaling_factor"), Some(Value::Double(_)))
        {
            return Err(MappingError::illegal_argument(
                "Field [scaling_factor] is required",
            ));
        }
        match self.ty {
            FieldType::Text => {
                let gap = self.get("position_increment_gap");
                let options = self
                    .get("index_options")
                    .and_then(Value::text)
                    .unwrap_or("positions");
                if gap != Some(&Value::Int(POSITION_INCREMENT_GAP_USE_ANALYZER))
                    && matches!(options, "docs" | "freqs")
                    && self.get("index").is_some_and(Value::flag)
                {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::illegal_argument(&format!(
                        "Cannot set position_increment_gap on field [{name}] without positions enabled"
                    )));
                }
                for analyzer in ["analyzer", "search_analyzer", "search_quote_analyzer"] {
                    let a = self
                        .get(analyzer)
                        .and_then(Value::text)
                        .unwrap_or("default");
                    // A pin's name is admitted here by its form; `Mapping::parse_with`
                    // refuses it to a caller whose index does not pin analyzers.
                    if !matches!(a, "default" | "standard") && !oss_ext::pin::is_pin_name(a) {
                        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                        return Err(MappingError::unsupported(&format!(
                            "analyzer [{a}] on field [{name}]"
                        )));
                    }
                }
            }
            FieldType::Date => {
                let format = self
                    .get("format")
                    .and_then(Value::text)
                    .unwrap_or(DEFAULT_DATE_FORMAT);
                if super::dates::Formats::parse(format).is_none() {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::unsupported(&format!(
                        "date format [{format}] on field [{name}]"
                    )));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Writes the field's body (inside its name) as `toXContent` does
    /// without `include_defaults`.
    pub fn write_body(&self, out: &mut String) {
        out.push_str("{\"type\":");
        write_str(out, self.ty.name());
        let defs = self.ty.defs();
        if self.ty == FieldType::Text {
            for name in TEXT_ORDER {
                if *name == "fields" {
                    self.write_multi_fields(out);
                } else if let Some(i) = defs.iter().position(|d| d.name == *name) {
                    self.write_param(i, out);
                }
            }
        } else {
            for i in 0..defs.len() {
                self.write_param(i, out);
            }
            self.write_multi_fields(out);
        }
        out.push('}');
    }

    fn write_multi_fields(&self, out: &mut String) {
        if self.fields.is_empty() {
            return;
        }
        out.push_str(",\"fields\":{");
        for (n, (name, leaf)) in self.fields.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            write_str(out, name);
            out.push(':');
            leaf.write_body(out);
        }
        out.push('}');
    }

    fn write_param(&self, i: usize, out: &mut String) {
        let defs = self.ty.defs();
        let (Some(def), Some(value)) = (defs.get(i), self.values.get(i)) else {
            return;
        };
        let write = match (self.ty, def.name) {
            // `TextParams.Analyzers`' serializer checks.
            (FieldType::Text, "analyzer") => {
                self.configured(i)
                    || Some(value) != self.get("search_analyzer")
                    || Some(value) != self.get("search_quote_analyzer")
            }
            (FieldType::Text, "search_analyzer") => {
                self.configured(i) || Some(value) != self.get("search_quote_analyzer")
            }
            _ => self.configured(i),
        };
        if !write {
            return;
        }
        out.push(',');
        write_str(out, def.name);
        out.push(':');
        value.write_json(out);
    }

    /// Merges `incoming` into this field as `ParametrizedFieldMapper.merge`
    /// does: same type, every parameter updateable or unchanged, multi-fields
    /// added or merged by name. `full_name` names it in messages.
    ///
    /// # Errors
    ///
    /// `IllegalArgumentException` with the Java's message for a changed type
    /// or a conflicting parameter.
    pub fn merge(&self, incoming: &Leaf, full_name: &str) -> Result<Leaf, MappingError> {
        if self.ty != incoming.ty {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::illegal_argument(&format!(
                "mapper [{full_name}] cannot be changed from type [{}] to [{}]",
                self.ty.name(),
                incoming.ty.name()
            )));
        }
        let mut merged = self.clone();
        // ALLOC: error path; one line a conflicting parameter.
        let mut conflicts = String::new();
        for (i, def) in self.ty.defs().iter().enumerate() {
            let (Some(current), Some(next)) = (self.values.get(i), incoming.values.get(i)) else {
                continue;
            };
            let ok = match def.merge {
                Merge::Updateable => true,
                Merge::Fixed => current == next,
                // Norms can be turned off, never back on.
                Merge::Norms => current.flag() || !next.flag(),
            };
            if ok {
                merged.values[i] = next.clone();
            } else {
                let _ = write!(
                    conflicts,
                    "\n\tCannot update parameter [{}] from [{}] to [{}]",
                    def.name,
                    current.conflict_text(),
                    next.conflict_text()
                );
            }
        }
        if !conflicts.is_empty() {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::illegal_argument(&format!(
                "Mapper for [{full_name}] conflicts with existing mapper:{conflicts}"
            )));
        }
        for (name, sub) in &incoming.fields {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            let sub_full = format!("{full_name}.{name}");
            match merged.fields.iter_mut().find(|(n, _)| n == name) {
                Some((_, existing)) => *existing = existing.merge(sub, &sub_full)?,
                // ALLOC: one name a new multi-field; mapping-update path.
                None => merged.fields.push((name.clone(), sub.clone())),
            }
        }
        merged.fields.sort_by(|a, b| java_cmp(&a.0, &b.0));
        Ok(merged)
    }
}

/// The type a mapping names, as `parserContext.typeParser(type)` finds it.
pub fn resolve_type(name: &str, field: &str, no_handler: &str) -> Result<FieldType, MappingError> {
    match FieldType::from_name(name) {
        Some(t) => Ok(t),
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        None if OTHER_TYPES.contains(&name) => Err(MappingError::unsupported(&format!(
            "field type [{name}] on field [{field}]"
        ))),
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        None => Err(MappingError::mapper_parsing(&format!(
            "{no_handler} [{name}] declared on field [{field}]"
        ))),
    }
}

fn fixed_default(d: D, kind: Kind) -> Value {
    match d {
        D::Null => Value::Null,
        D::Bool(b) if kind == Kind::Explicit => Value::Explicit(b, false),
        D::Bool(b) => Value::Bool(b),
        D::Int(i) => Value::Int(i),
        D::Float(f) => Value::Float(f),
        // ALLOC: a parameter's default text; once a parameter a field.
        D::Str(s) => Value::Str(s.to_owned()),
        // ALLOC: none; an empty map.
        D::Empty => Value::Meta(Vec::new()),
        D::Same(_) => Value::Null,
    }
}
