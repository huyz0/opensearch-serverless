//! The recursive descent over a document, ported from `DocumentParser`'s
//! `innerParseObject`, `parseObject`, `parseArray`, `parseValue`,
//! `parseNullValue`, `parseDynamicValue` and `getDynamicParentMapper`.

use crate::mapping::json::Json;
use crate::mapping::{
    Dynamic, FieldType, Leaf, Mapper, Mapping, MappingError, NumType, Object, Value,
};

use super::preview::preview;
use super::reader::{Reader, Tok};
use super::values::{Taken, take};
use super::{ParsedDocument, dynamic_object, split_path};

/// `index.mapping.depth.limit`'s default, for objects and for arrays.
const DEPTH_LIMIT: usize = 20;

/// Metadata fields a document body may not hold.
const METADATA_FIELDS: [&str; 10] = [
    "_id",
    "_index",
    "_routing",
    "_source",
    "_seq_no",
    "_primary_term",
    "_version",
    "_field_names",
    "_ignored",
    "_nested_path",
];

/// The metadata field a document may set: `DocCountFieldMapper`.
const DOC_COUNT: &str = "_doc_count";

/// The object a key is read in.
#[derive(Clone, Copy)]
pub(super) struct Scope<'m> {
    /// The object's mapping; `None` for one dynamic mapping is adding.
    pub(super) obj: Option<&'m Object>,
    /// Its own `dynamic`, as `ObjectMapper.dynamic()` returns it.
    pub(super) explicit: Option<Dynamic>,
    /// Its `dynamic` with the parents' applied (`dynamicOrDefault`).
    pub(super) dynamic: Dynamic,
    /// The root's scope, whose full path is `_doc`.
    pub(super) root: bool,
}

/// One document's parse.
pub struct Parser<'m, 'a> {
    pub(super) mapping: &'m Mapping,
    pub(super) id: &'a str,
    pub(super) reader: Reader<'a>,
    /// `context.path()`, one segment an object entered.
    pub(super) path: Vec<String>,
    pub(super) depth: usize,
    pub(super) array_depth: usize,
    pub(super) out: ParsedDocument,
    pub(super) update: Object,
    /// Fields the mapping had, and fields the update adds, as the total
    /// fields limit counts them.
    pub(super) existing: usize,
    pub(super) added: usize,
    pub(super) conflict: Option<MappingError>,
}

impl<'m, 'a> Parser<'m, 'a> {
    /// A parse of `source` against `mapping`.
    pub fn new(mapping: &'m Mapping, id: &'a str, source: &'a [u8]) -> Self {
        Self {
            mapping,
            id,
            reader: Reader::new(source),
            // ALLOC: one segment a nesting level, at most DEPTH_LIMIT.
            path: Vec::new(),
            depth: 0,
            array_depth: 0,
            out: ParsedDocument::default(),
            update: dynamic_object(),
            existing: mapping.field_count(),
            added: 0,
            conflict: None,
        }
    }

    /// The values, the dynamic additions, and the first conflict between
    /// two of them.
    pub fn finish(self) -> (ParsedDocument, Object, Option<MappingError>) {
        (self.out, self.update, self.conflict)
    }

    /// `validateStart`, the root object, then `validateEnd`.
    pub fn document(&mut self) -> Result<(), MappingError> {
        if self.reader.next()? != Tok::BeginObject {
            // The content type is detected from a leading `{` before any
            // parsing, so anything else fails there.
            return Err(MappingError::not_x_content(
                "Compressor detection can only be called on some xcontent bytes or compressed xcontent bytes",
            ));
        }
        let root = self.mapping.root();
        if root.enabled {
            let scope = Scope {
                obj: Some(root),
                explicit: root.dynamic,
                dynamic: root.dynamic.unwrap_or(Dynamic::True),
                root: true,
            };
            self.inner_object(scope)?;
        } else {
            self.reader.skip_children(&Tok::BeginObject)?;
        }
        match self.reader.next()? {
            Tok::Eof => Ok(()),
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            t => Err(MappingError::illegal_argument(&format!(
                "Malformed content, found extra data after parsing: {}",
                t.jackson_name()
            ))),
        }
    }

    pub(super) fn path_text(&self) -> String {
        self.path.join(".")
    }

    /// `fullPath()` of the scope: `_doc` for the root.
    pub(super) fn full(&self, scope: Scope<'_>) -> String {
        if scope.root {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            "_doc".to_owned()
        } else {
            self.path_text()
        }
    }

    fn push(&mut self, parts: &[&str]) -> usize {
        for part in parts {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            self.path.push((*part).to_owned());
        }
        parts.len()
    }

    fn pop(&mut self, n: usize) {
        for _ in 0..n {
            self.path.pop();
        }
    }

    /// `innerParseObject`, after its `{`.
    fn inner_object(&mut self, scope: Scope<'m>) -> Result<(), MappingError> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > DEPTH_LIMIT {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            return Err(MappingError::opensearch_parse(&format!(
                "The depth of the field has exceeded the allowed limit of [{DEPTH_LIMIT}]. This limit can be set by changing the [index.mapping.depth.limit] index level setting."
            )));
        }
        loop {
            let key = match self.reader.next()? {
                Tok::EndObject => break,
                Tok::Key(k) => k,
                t => {
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    return Err(MappingError::json_parse(&format!(
                        "unexpected {}",
                        t.jackson_name()
                    )));
                }
            };
            let value = self.reader.next()?;
            let parts = split_path(&key)?;
            if contains_disabled(scope.obj, &parts) {
                self.reader.skip_children(&value)?;
                continue;
            }
            if scope.root && parts.len() == 1 {
                if METADATA_FIELDS.contains(&key.as_str()) {
                    // `MetadataFieldMapper.parse` refuses it inside `FieldMapper.parse`.
                    let shown = preview(&self.reader.read_value(value)?, false);
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    return Err(MappingError::mapper_parsing(&format!(
                        "failed to parse field [{key}] of type [{key}] in document with id '{}'. Preview of field's value: '{shown}'",
                        self.id
                    ))
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    .caused_by(MappingError::mapper_parsing(&format!(
                        "Field [{key}] is a metadata field and cannot be added inside a document. Use the index API request parameters."
                    ))));
                }
                if key == DOC_COUNT {
                    // `_doc_count` is a metadata field a document may set;
                    // the writer does not index it yet.
                    self.reader.skip_children(&value)?;
                    continue;
                }
            }
            match value {
                Tok::BeginObject => self.parse_object(scope, &key, &parts)?,
                Tok::BeginArray => self.parse_array(scope, &key, &parts)?,
                Tok::Null => self.parse_null(scope, &key, &parts)?,
                t => self.parse_value(scope, &key, &parts, t)?,
            }
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    /// `parseObject`, at its `{`.
    fn parse_object(
        &mut self,
        scope: Scope<'m>,
        key: &str,
        parts: &[&str],
    ) -> Result<(), MappingError> {
        if let Some(mapper) = get_mapper(scope.obj, parts) {
            let n = self.push(parts);
            self.object_or_field(scope, mapper, key, Tok::BeginObject)?;
            self.pop(n);
            return Ok(());
        }
        let name = parts.last().copied().unwrap_or(key);
        let (added, parent) = self.dynamic_parent(scope, parts)?;
        match parent.dynamic {
            Dynamic::Strict | Dynamic::StrictAllowTemplates => {
                return Err(MappingError::strict_dynamic_as(
                    parent.dynamic.name(),
                    &self.full_before(scope, added),
                    name,
                ));
            }
            Dynamic::True => {
                self.add_dynamic(name, Mapper::Object(dynamic_object()));
                let n = self.push(&[name]);
                let created = Scope {
                    obj: None,
                    explicit: None,
                    dynamic: Dynamic::True,
                    root: false,
                };
                self.inner_object(created)?;
                self.pop(n);
            }
            Dynamic::False | Dynamic::FalseAllowTemplates => {
                self.reader.skip_children(&Tok::BeginObject)?;
            }
        }
        self.pop(added);
        Ok(())
    }

    /// The scope's full path with the `added` segments `dynamic_parent`
    /// pushed taken off: `mapper.fullPath()` of the object being read.
    fn full_before(&self, scope: Scope<'_>, added: usize) -> String {
        if scope.root {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            return "_doc".to_owned();
        }
        let keep = self.path.len().saturating_sub(added);
        self.path.get(..keep).unwrap_or_default().join(".")
    }

    /// `parseObjectOrField` for a mapper found in the mapping.
    fn object_or_field(
        &mut self,
        parent: Scope<'m>,
        mapper: &'m Mapper,
        key: &str,
        tok: Tok,
    ) -> Result<(), MappingError> {
        match mapper {
            Mapper::Object(o) => {
                if !o.enabled {
                    return self.reader.skip_children(&tok);
                }
                match tok {
                    Tok::Null => Ok(()),
                    Tok::BeginObject => {
                        let scope = Scope {
                            obj: Some(o),
                            explicit: o.dynamic,
                            dynamic: o.dynamic.unwrap_or(parent.dynamic),
                            root: false,
                        };
                        self.inner_object(scope)
                    }
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    _ => Err(MappingError::mapper_parsing(&format!(
                        "object mapping for [{}] tried to parse field [{key}] as object, but found a concrete value",
                        self.path_text()
                    ))),
                }
            }
            Mapper::Leaf(leaf) => {
                let full = self.path_text();
                self.parse_field(leaf, &full, tok)
            }
        }
    }

    /// `FieldMapper.parse`: the value, the Java's wrapping of a failure,
    /// `ignore_malformed`, and the multi-fields.
    pub(super) fn parse_field(
        &mut self,
        leaf: &Leaf,
        full: &str,
        tok: Tok,
    ) -> Result<(), MappingError> {
        let at = self.reader.at();
        match take(leaf, &tok, at) {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            Ok(Taken::Value(v)) => self.out.fields.push((full.to_owned(), v)),
            Ok(Taken::Nothing) => {}
            Err(cause) => {
                let value = self.reader.read_value(tok)?;
                if leaf.get("ignore_malformed").is_some_and(Value::flag) {
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    self.out.ignored.push(full.to_owned());
                    return Ok(());
                }
                let float = leaf.ty() == FieldType::Number(NumType::Float);
                let shown = if value == Json::Null {
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    "null".to_owned()
                } else {
                    preview(&value, float)
                };
                // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                return Err(MappingError::mapper_parsing(&format!(
                    "failed to parse field [{full}] of type [{}] in document with id '{}'. Preview of field's value: '{shown}'",
                    leaf.ty().name(),
                    self.id
                ))
                .caused_by(cause));
            }
        }
        for (name, sub) in leaf.multi_fields() {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            let sub_full = format!("{full}.{name}");
            self.parse_field(sub, &sub_full, tok.clone())?;
        }
        Ok(())
    }

    /// `parseArray`, at its `[`.
    fn parse_array(
        &mut self,
        scope: Scope<'m>,
        key: &str,
        parts: &[&str],
    ) -> Result<(), MappingError> {
        self.array_depth = self.array_depth.saturating_add(1);
        if self.array_depth > DEPTH_LIMIT {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            return Err(MappingError::opensearch_parse(&format!(
                "The depth of the nested array field has exceeded the allowed limit of [{DEPTH_LIMIT}]. This limit can be set by changing the [index.mapping.depth.limit] index level setting."
            )));
        }
        if get_mapper(scope.obj, parts).is_some() {
            self.non_dynamic_array(scope, key)?;
        } else {
            let name = parts.last().copied().unwrap_or(key);
            let (added, parent) = self.dynamic_parent(scope, parts)?;
            match parent.dynamic {
                Dynamic::Strict | Dynamic::StrictAllowTemplates => {
                    return Err(MappingError::strict_dynamic_as(
                        parent.dynamic.name(),
                        &self.full(parent),
                        name,
                    ));
                }
                Dynamic::FalseAllowTemplates => self.reader.skip_children(&Tok::BeginArray)?,
                Dynamic::True | Dynamic::False => self.non_dynamic_array(parent, name)?,
            }
            self.pop(added);
        }
        self.array_depth = self.array_depth.saturating_sub(1);
        Ok(())
    }

    /// `parseNonDynamicArray`: each element as a value of `key`.
    fn non_dynamic_array(&mut self, scope: Scope<'m>, key: &str) -> Result<(), MappingError> {
        let parts = split_path(key)?;
        loop {
            match self.reader.next()? {
                Tok::EndArray => return Ok(()),
                Tok::BeginObject => self.parse_object(scope, key, &parts)?,
                Tok::BeginArray => self.parse_array(scope, key, &parts)?,
                Tok::Null => self.parse_null(scope, key, &parts)?,
                t => self.parse_value_named(scope, key, &parts, t, "null")?,
            }
        }
    }

    /// `parseNullValue`: only a mapped field, or a strict object's refusal.
    fn parse_null(
        &mut self,
        scope: Scope<'m>,
        key: &str,
        parts: &[&str],
    ) -> Result<(), MappingError> {
        if let Some(mapper) = get_mapper(scope.obj, parts) {
            let n = self.push(parts);
            self.object_or_field(scope, mapper, key, Tok::Null)?;
            self.pop(n);
            return Ok(());
        }
        if let Some(d @ (Dynamic::Strict | Dynamic::StrictAllowTemplates)) = scope.explicit {
            return Err(MappingError::strict_dynamic_as(
                d.name(),
                &self.full(scope),
                key,
            ));
        }
        Ok(())
    }

    /// `parseValue`: a scalar.
    fn parse_value(
        &mut self,
        scope: Scope<'m>,
        key: &str,
        parts: &[&str],
        tok: Tok,
    ) -> Result<(), MappingError> {
        self.parse_value_named(scope, key, parts, tok, key)
    }

    /// `parseValue`, where the parser's `currentName()` is `current`:
    /// the key, or `null` for an element of an array.
    fn parse_value_named(
        &mut self,
        scope: Scope<'m>,
        key: &str,
        parts: &[&str],
        tok: Tok,
        current: &str,
    ) -> Result<(), MappingError> {
        if let Some(mapper) = get_mapper(scope.obj, parts) {
            let n = self.push(parts);
            self.object_or_field(scope, mapper, current, tok)?;
            self.pop(n);
            return Ok(());
        }
        let name = parts.last().copied().unwrap_or(key);
        let (added, parent) = self.dynamic_parent(scope, parts)?;
        self.dynamic_value(parent, name, tok)?;
        self.pop(added);
        Ok(())
    }
}

/// `getMapper`: a child, through the objects a dotted name's leading parts
/// name.
fn get_mapper<'m>(obj: Option<&'m Object>, parts: &[&str]) -> Option<&'m Mapper> {
    let mut obj = obj?;
    let (last, leading) = parts.split_last()?;
    for part in leading {
        match obj.child(part) {
            Some(Mapper::Object(o)) => obj = o,
            _ => return None,
        }
    }
    obj.child(last)
}

/// `containsDisabledObjectMapper`.
fn contains_disabled(obj: Option<&Object>, parts: &[&str]) -> bool {
    let Some(mut obj) = obj else {
        return false;
    };
    let leading = parts.len().saturating_sub(1);
    for part in parts.iter().take(leading) {
        match obj.child(part) {
            Some(Mapper::Object(o)) => {
                if !o.enabled {
                    return true;
                }
                obj = o;
            }
            _ => break,
        }
    }
    false
}
