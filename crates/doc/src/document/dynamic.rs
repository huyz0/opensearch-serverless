//! Dynamic mapping: the fields and objects a document adds, ported from
//! `DocumentParser.parseDynamicValue`, `createBuilderFromDynamicValue`,
//! `getDynamicParentMapper`, `dynamicOrDefault` and `createDynamicUpdate`.

use crate::mapping::json::is_integral;
use crate::mapping::{
    Dynamic, FieldType, Formats, Leaf, Mapper, Mapping, MappingError, NumType, Object, Value,
    java_parse_double,
};

use super::dynamic_object;
use super::reader::Tok;
use super::walk::{Parser, Scope};

/// `index.mapping.total_fields.limit`'s default.
const TOTAL_FIELDS_LIMIT: usize = 1000;

/// The formats dynamic mapping tries on a string (`dynamic_date_formats`).
const DYNAMIC_DATE_FORMATS: [&str; 2] = [
    crate::mapping::DEFAULT_DATE_FORMAT,
    "yyyy/MM/dd HH:mm:ss||yyyy/MM/dd||epoch_millis",
];

impl<'m> Parser<'m, '_> {
    /// `parseDynamicValue`: a new field from a value's kind.
    pub(super) fn dynamic_value(
        &mut self,
        parent: Scope<'m>,
        name: &str,
        tok: Tok,
    ) -> Result<(), MappingError> {
        match parent.dynamic {
            Dynamic::Strict | Dynamic::StrictAllowTemplates => Err(
                MappingError::strict_dynamic_as(parent.dynamic.name(), &self.full(parent), name),
            ),
            Dynamic::False | Dynamic::FalseAllowTemplates => Ok(()),
            Dynamic::True => {
                let leaf = self.dynamic_leaf(&tok);
                self.add_dynamic(name, Mapper::Leaf(leaf.clone()));
                let full = if self.path.is_empty() {
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    name.to_owned()
                } else {
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    format!("{}.{name}", self.path_text())
                };
                self.parse_field(&leaf, &full, tok)
            }
        }
    }

    /// `createBuilderFromDynamicValue` with no dynamic templates.
    fn dynamic_leaf(&self, tok: &Tok) -> Leaf {
        match tok {
            Tok::Str(text) => {
                let as_long = java_parse_long(text);
                let as_double = java_parse_double(text).is_some();
                if as_long && self.mapping.numeric_detection() {
                    return Leaf::new(FieldType::Number(NumType::Long));
                }
                if as_double && self.mapping.numeric_detection() {
                    return Leaf::new(FieldType::Number(NumType::Float));
                }
                if !as_long && !as_double && self.mapping.date_detection() {
                    for (i, pattern) in DYNAMIC_DATE_FORMATS.iter().enumerate() {
                        let reads = Formats::parse(pattern).is_some_and(|f| {
                            f.invalid_day(text).is_none() && f.parse_millis(text).is_some()
                        });
                        if reads {
                            let mut leaf = Leaf::new(FieldType::Date);
                            if i > 0 {
                                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                                leaf.set("format", Value::Str((*pattern).to_owned()));
                                leaf.set(
                                    "print_format",
                                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                                    Value::Str("yyyy/MM/dd HH:mm:ss".to_owned()),
                                );
                            }
                            return leaf;
                        }
                    }
                }
                let mut text_leaf = Leaf::new(FieldType::Text);
                let mut keyword = Leaf::new(FieldType::Keyword);
                keyword.set("ignore_above", Value::Int(256));
                text_leaf.add_multi_field("keyword", keyword);
                text_leaf
            }
            Tok::Num(t) if is_integral(t) => Leaf::new(FieldType::Number(NumType::Long)),
            Tok::Num(_) => Leaf::new(FieldType::Number(NumType::Float)),
            _ => Leaf::new(FieldType::Boolean),
        }
    }

    /// `getDynamicParentMapper`: the objects a dotted name's leading parts
    /// name, added when the mapping lacks them and `dynamic` allows.
    /// Returns how many path segments it pushed, and the last object.
    pub(super) fn dynamic_parent(
        &mut self,
        scope: Scope<'m>,
        parts: &[&str],
    ) -> Result<(usize, Scope<'m>), MappingError> {
        let mut parent = scope;
        let mut added = 0usize;
        let leading = parts.len().saturating_sub(1);
        for part in parts.iter().take(leading) {
            let here = if self.path.is_empty() {
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                (*part).to_owned()
            } else {
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                format!("{}.{part}", self.path_text())
            };
            let next = match lookup(self.mapping.root(), &here) {
                Some(Mapper::Leaf(leaf)) => {
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    return Err(MappingError::mapper_parsing(&format!(
                        "Could not dynamically add mapping for field [{}]. Existing mapping for [{here}] must be of type object but found [{}].",
                        parts.join("."),
                        leaf.ty().name()
                    )));
                }
                Some(Mapper::Object(o)) => Scope {
                    obj: Some(o),
                    explicit: o.dynamic,
                    dynamic: effective_dynamic(self.mapping, &here),
                    root: false,
                },
                None => match parent.dynamic {
                    Dynamic::Strict | Dynamic::StrictAllowTemplates => {
                        return Err(MappingError::strict_dynamic_as(
                            parent.dynamic.name(),
                            &self.full(parent),
                            part,
                        ));
                    }
                    Dynamic::True => {
                        self.add_dynamic(part, Mapper::Object(dynamic_object()));
                        Scope {
                            obj: None,
                            explicit: None,
                            dynamic: Dynamic::True,
                            root: false,
                        }
                    }
                    Dynamic::False | Dynamic::FalseAllowTemplates => return Ok((added, parent)),
                },
            };
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            self.path.push((*part).to_owned());
            added = added.saturating_add(1);
            parent = next;
        }
        Ok((added, parent))
    }

    /// `context.addDynamicMapper`, then `createDynamicUpdate`'s merge,
    /// done in place as each mapper is added. The first conflict is kept
    /// for after the parse, as the Java merges after it; past the total
    /// fields limit nothing more is added, so the update never holds more
    /// than the limit allows.
    pub(super) fn add_dynamic(&mut self, name: &str, mapper: Mapper) {
        if self.conflict.is_some() {
            return;
        }
        let full = if self.path.is_empty() {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            name.to_owned()
        } else {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            format!("{}.{name}", self.path_text())
        };
        let mut obj = &mut self.update;
        let mut prefix = String::new();
        for segment in &self.path {
            if !prefix.is_empty() {
                prefix.push('.');
            }
            prefix.push_str(segment);
            if obj.child(segment).is_none() {
                // An existing nested object keeps its kind in the update, as the Java's
                // `mappingUpdate` of a parent does, or the merge would change it.
                let mut parent = dynamic_object();
                parent.nested = self.mapping.is_nested(&prefix);
                obj.put(segment, Mapper::Object(parent));
            }
            obj = match obj.child_mut(segment) {
                Some(Mapper::Object(o)) => o,
                // A leaf here: adding the object at this segment already
                // recorded the conflict, so nothing is added.
                _ => return,
            };
        }
        let fields = match &mapper {
            Mapper::Leaf(leaf) => leaf.multi_fields().len().saturating_add(1),
            Mapper::Object(_) => 1,
        };
        let merged = match (obj.child(name), mapper) {
            (None, m) => {
                self.added = self.added.saturating_add(fields);
                m
            }
            (Some(Mapper::Object(_)), Mapper::Object(_)) => return,
            (Some(Mapper::Leaf(have)), Mapper::Leaf(new)) => match have.merge(&new, &full) {
                Ok(l) => Mapper::Leaf(l),
                Err(e) => {
                    self.conflict = Some(e);
                    return;
                }
            },
            (Some(_), _) => {
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                self.conflict = Some(MappingError::illegal_argument(&format!(
                    "can't merge a non object mapping [{full}] with an object mapping"
                )));
                return;
            }
        };
        obj.put(name, merged);
        if self.existing.saturating_add(self.added) > TOTAL_FIELDS_LIMIT {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            self.conflict = Some(MappingError::illegal_argument(&format!(
                "Limit of total fields [{TOTAL_FIELDS_LIMIT}] has been exceeded"
            )));
        }
    }
}

/// The mapper at a full dotted path, as `mappers().getMapper` and
/// `objectMappers().get` find one; a leaf met on the way is returned.
fn lookup<'m>(root: &'m Object, full: &str) -> Option<&'m Mapper> {
    let mut obj = root;
    let mut found = None;
    for part in full.split('.') {
        match obj.child(part)? {
            m @ Mapper::Object(o) => {
                obj = o;
                found = Some(m);
            }
            m @ Mapper::Leaf(_) => return Some(m),
        }
    }
    found
}

/// `dynamicOrDefault` for the existing object at `full`: its own setting,
/// else the nearest parent's, else the root's, else `true`.
fn effective_dynamic(mapping: &Mapping, full: &str) -> Dynamic {
    let root = mapping.root();
    let mut dynamic = root.dynamic.unwrap_or(Dynamic::True);
    let mut obj = root;
    for part in full.split('.') {
        match obj.child(part) {
            Some(Mapper::Object(o)) => {
                if let Some(d) = o.dynamic {
                    dynamic = d;
                }
                obj = o;
            }
            _ => break,
        }
    }
    dynamic
}

/// `Long.parseLong` succeeds: an optional sign, then digits, in range.
fn java_parse_long(text: &str) -> bool {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && text.parse::<i64>().is_ok()
}
