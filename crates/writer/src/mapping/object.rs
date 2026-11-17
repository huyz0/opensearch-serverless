//! Object fields and the tree of a mapping's properties.
//!
//! Ported from `ObjectMapper` (server, OpenSearch 3.10): `TypeParser.parse`
//! and `parseProperties` (dotted names become nested objects), `doMerge`
//! and `toXContent` (children sorted by full name, `type` written only for
//! an object without children). Nested objects, `disable_objects`, derived
//! fields and composite fields are refused as unsupported.

use super::error::MappingError;
use super::field::{FieldType, Leaf, resolve_type};
use super::json::Json;
use super::json::{java_cmp, write_str};
use super::params::named_bool;

/// An object's `dynamic`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dynamic {
    /// New fields are mapped.
    True,
    /// New fields are kept in `_source` and not mapped.
    False,
    /// A document with a new field is refused.
    Strict,
    /// `strict_allow_templates`: refused unless a dynamic template maps it.
    StrictAllowTemplates,
    /// `false_allow_templates`: not mapped unless a dynamic template maps it.
    FalseAllowTemplates,
}

impl Dynamic {
    /// The name `toXContent` writes: `Dynamic.name()` in lower case.
    pub fn name(self) -> &'static str {
        match self {
            Dynamic::True => "true",
            Dynamic::False => "false",
            Dynamic::Strict => "strict",
            Dynamic::StrictAllowTemplates => "strict_allow_templates",
            Dynamic::FalseAllowTemplates => "false_allow_templates",
        }
    }

    /// Reads `dynamic` as `parseObjectOrDocumentTypeProperties` does.
    pub fn parse(node: &Json) -> Result<Self, MappingError> {
        let text = node.java_string();
        Ok(match text.to_ascii_lowercase().as_str() {
            "strict" => Dynamic::Strict,
            "strict_allow_templates" => Dynamic::StrictAllowTemplates,
            "false_allow_templates" => Dynamic::FalseAllowTemplates,
            _ => {
                if named_bool("dynamic.dynamic", node)? {
                    Dynamic::True
                } else {
                    Dynamic::False
                }
            }
        })
    }
}

/// Writes the root's own options into its body; the flag says whether
/// nothing was written yet.
pub type RootExtra<'a> = dyn Fn(&mut String, &mut bool) + 'a;

/// A node of the properties tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Mapper {
    /// A leaf field.
    Leaf(Leaf),
    /// An object.
    Object(Object),
}

/// An object field, or the root.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object {
    /// `dynamic`, or `None` to inherit the parent's.
    pub dynamic: Option<Dynamic>,
    /// `enabled`: false keeps the object in `_source` and indexes nothing
    /// under it.
    pub enabled: bool,
    /// Whether `enabled` was written (`Explicit<Boolean>`).
    pub enabled_explicit: bool,
    /// Children by simple name, in Java's `String` order.
    children: Vec<(String, Mapper)>,
}

impl Object {
    /// An empty, enabled object that inherits `dynamic`.
    pub fn new() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    /// The children by simple name, sorted.
    pub fn children(&self) -> &[(String, Mapper)] {
        &self.children
    }

    /// A child by simple name.
    pub fn child(&self, name: &str) -> Option<&Mapper> {
        self.children
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, m)| m)
    }

    /// Adds or replaces a child, keeping the order.
    pub fn put(&mut self, name: &str, mapper: Mapper) {
        match self.children.binary_search_by(|(n, _)| java_cmp(n, name)) {
            Ok(i) => {
                if let Some(slot) = self.children.get_mut(i) {
                    slot.1 = mapper;
                }
            }
            // ALLOC: one name a new child; mapping parse and update paths.
            Err(i) => self.children.insert(i, (name.to_owned(), mapper)),
        }
    }

    /// Reads an object's own options and properties from its mapping
    /// object, leaving keys it does not know in `rest`. `name` is its full
    /// name, empty for the root.
    pub fn parse_into(
        &mut self,
        name: &str,
        node: &[(String, Json)],
        rest: &mut Vec<(String, Json)>,
    ) -> Result<(), MappingError> {
        for (key, value) in node {
            match key.as_str() {
                "dynamic" => self.dynamic = Some(Dynamic::parse(value)?),
                "enabled" => {
                    self.enabled = named_bool("enabled.enabled", value)?;
                    self.enabled_explicit = true;
                }
                "properties" => match value {
                    Json::Obj(props) => self.parse_properties(name, props)?,
                    Json::Arr(a) if a.is_empty() => {}
                    _ => return Err(MappingError::parse("properties must be a map type")),
                },
                "include_in_all" => {}
                "disable_objects" | "derived" | "composite" | "context_aware_grouping" => {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::unsupported(&format!(
                        "[{key}] on object [{name}]"
                    )));
                }
                // ALLOC: one entry a key left for the caller; parse path.
                _ => rest.push((key.clone(), value.clone())),
            }
        }
        Ok(())
    }

    fn parse_properties(
        &mut self,
        parent: &str,
        props: &[(String, Json)],
    ) -> Result<(), MappingError> {
        for (field, value) in props {
            let Json::Obj(node) = value else {
                if matches!(value, Json::Arr(a) if a.is_empty()) {
                    continue;
                }
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "Expected map for property [fields] on field [{field}] but got a class java.lang.String"
                )));
            };
            let ty = match value.get("type") {
                Some(t) => t.java_string(),
                None if value.get("properties").is_some() => "object".into(),
                None if node.len() == 1 && value.get("enabled").is_some() => "object".into(),
                None => {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::mapper_parsing(&format!(
                        "No type specified for field [{field}]"
                    )));
                }
            };
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            let parts: Vec<&str> = field.split('.').collect();
            // `split` yields at least one part, so the fallback is never taken.
            let (last, parents) = parts
                .split_last()
                .map_or((field.as_str(), &[][..]), |(l, p)| (*l, p));
            let full = join(parent, field);
            let mapper = if ty == "object" {
                let mut object = Object::new();
                // ALLOC: the keys an object's mapping leaves; parse path.
                let mut rest = Vec::new();
                object.parse_into(&full, node, &mut rest)?;
                rest.retain(|(k, _)| k != "type");
                if !rest.is_empty() {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::mapper_parsing(&format!(
                        "Mapping definition for [{field}] has unsupported parameters: {}",
                        remaining(&rest)
                    )));
                }
                if last.is_empty() {
                    return Err(MappingError::illegal_argument(
                        "name cannot be empty string",
                    ));
                }
                Mapper::Object(object)
            } else {
                let ty: FieldType = resolve_type(&ty, field, "No handler for type")?;
                Mapper::Leaf(Leaf::parse(last, ty, node, false)?)
            };
            // `a.b.c` is `a` holding `b` holding `c`.
            let mut mapper = mapper;
            for i in (0..parents.len()).rev() {
                let mut holder = Object::new();
                let held = parts.get(i.saturating_add(1)).copied().unwrap_or(last);
                holder.put(held, mapper);
                mapper = Mapper::Object(holder);
            }
            let top = parts.first().copied().unwrap_or(last);
            self.add_parsed(top, mapper)?;
        }
        Ok(())
    }

    /// Adds a parsed child, merging it into one of the same name the way
    /// `ObjectMapper.Builder.add` keeps the last (a later dotted name
    /// extends an earlier object).
    fn add_parsed(&mut self, name: &str, mapper: Mapper) -> Result<(), MappingError> {
        match (self.child(name).cloned(), mapper) {
            (Some(Mapper::Object(existing)), Mapper::Object(new)) => {
                let merged = existing.merge(&new, name)?;
                self.put(name, Mapper::Object(merged));
            }
            (_, mapper) => self.put(name, mapper),
        }
        Ok(())
    }

    /// Merges `incoming` into this object as `ObjectMapper.doMerge` does.
    /// `full_name` names it in messages.
    ///
    /// # Errors
    ///
    /// The Java's messages for an object turned into a field or back, a
    /// changed `enabled`, and every conflict of the fields under it.
    pub fn merge(&self, incoming: &Object, full_name: &str) -> Result<Object, MappingError> {
        let mut merged = self.clone();
        if incoming.dynamic.is_some() {
            merged.dynamic = incoming.dynamic;
        }
        if self.enabled != incoming.enabled {
            // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
            return Err(MappingError::mapper(&format!(
                "the [enabled] parameter can't be updated for the object mapping [{full_name}]"
            )));
        }
        for (name, mapper) in &incoming.children {
            let child_full = join(full_name, name);
            let next = match (merged.child(name), mapper) {
                (None, m) => m.clone(),
                (Some(Mapper::Object(existing)), Mapper::Object(new)) => {
                    Mapper::Object(existing.merge(new, &child_full)?)
                }
                (Some(Mapper::Object(_)), Mapper::Leaf(_))
                | (Some(Mapper::Leaf(_)), Mapper::Object(_)) => {
                    // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                    return Err(MappingError::illegal_argument(&format!(
                        "can't merge a non object mapping [{child_full}] with an object mapping"
                    )));
                }
                (Some(Mapper::Leaf(existing)), Mapper::Leaf(new)) => {
                    Mapper::Leaf(existing.merge(new, &child_full)?)
                }
            };
            merged.put(name, next);
        }
        Ok(merged)
    }

    /// Writes the object's body as `toXContent` does; `root_extra` writes
    /// the root's own options between `enabled` and `properties`.
    pub fn write_body(&self, out: &mut String, root_extra: Option<&RootExtra<'_>>) {
        out.push('{');
        let mut first = true;
        let sep = |out: &mut String, first: &mut bool| {
            if !*first {
                out.push(',');
            }
            *first = false;
        };
        if self.children.is_empty() && root_extra.is_none() {
            sep(out, &mut first);
            out.push_str("\"type\":\"object\"");
        }
        if let Some(d) = self.dynamic {
            sep(out, &mut first);
            out.push_str("\"dynamic\":");
            write_str(out, d.name());
        }
        if !self.enabled {
            sep(out, &mut first);
            out.push_str("\"enabled\":false");
        }
        if let Some(extra) = root_extra {
            extra(out, &mut first);
        }
        if !self.children.is_empty() {
            sep(out, &mut first);
            out.push_str("\"properties\":{");
            for (i, (name, mapper)) in self.children.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(out, name);
                out.push(':');
                match mapper {
                    Mapper::Leaf(leaf) => leaf.write_body(out),
                    Mapper::Object(object) => object.write_body(out, None),
                }
            }
            out.push('}');
        }
        out.push('}');
    }

    /// Visits every mapper under this object with its full name, objects
    /// before their children.
    pub fn walk<'a>(&'a self, prefix: &str, f: &mut dyn FnMut(&str, &'a Mapper)) {
        for (name, mapper) in &self.children {
            let full = join(prefix, name);
            f(&full, mapper);
            if let Mapper::Object(o) = mapper {
                o.walk(&full, f);
            }
        }
    }
}

/// `parent.name`, or `name` at the root.
pub fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        // ALLOC: one full name; mapping parse and update paths.
        name.to_owned()
    } else {
        format!("{parent}.{name}")
    }
}

/// `DocumentMapperParser.getRemainingFields`: ` [key : value]` each.
pub fn remaining(rest: &[(String, Json)]) -> String {
    // ALLOC: error path; one message.
    let mut out = String::new();
    for (k, v) in rest {
        out.push_str(" [");
        out.push_str(k);
        out.push_str(" : ");
        out.push_str(&v.java_string());
        out.push(']');
    }
    out
}
