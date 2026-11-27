//! An index's field types, read from the mapping in its descriptor.
//!
//! The shell keeps an index's mapping as a JSON string inside its
//! descriptor (`IndexDescriptor.toXContent`'s `mapping`). This decodes that
//! string with `oss_rest`'s tokenizer and reads each field's `type` and the
//! options that change how it is searched, walking `properties` of objects
//! and multi-fields (`fields`) into dotted names, as `MapperService` names
//! them. Only what a query of the subset needs is kept.

use std::collections::{BTreeMap, BTreeSet};

use oss_rest::json::{JsonError, Limits, Location, Token, TokenSink, Tokenizer};

use crate::dsl::tokens::{Kind, Parser, Recorder};

/// The most bytes a mapping's decoded text and tokens may take: the
/// descriptor's own limit (`MAX_REGISTER_BYTES`) twice over.
pub const MAX_MAPPING_BYTES: usize = 8 << 20;

/// The most fields a mapping may hold, `index.mapping.total_fields.limit`'s
/// default.
pub const MAX_FIELDS: usize = 1000;

/// How a field is searched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldType {
    /// `text`, analysed with the standard analyzer.
    Text,
    /// `keyword`, without a normalizer.
    Keyword,
    /// `byte`, `short` or `integer`: `IntPoint`s and sorted numeric doc
    /// values. The bounds are the type's range.
    Int {
        /// The type's name.
        name: &'static str,
        /// The smallest value.
        min: i64,
        /// The largest value.
        max: i64,
    },
    /// `long`: `LongPoint`s and sorted numeric doc values.
    Long,
    /// `date` in the default format (`strict_date_optional_time||epoch_millis`),
    /// as epoch milliseconds in `LongPoint`s and sorted numeric doc values.
    Date,
    /// `float`: `FloatPoint`s and doc values of the sortable int.
    Float,
    /// `double`: `DoublePoint`s and doc values of the sortable long.
    Double,
    /// `half_float`: 2-byte `HalfFloatPoint`s and doc values of the sortable short.
    HalfFloat,
    /// `scaled_float`: `LongPoint`s and doc values of `Math.round(value * scaling_factor)`;
    /// holds the bits of the factor.
    Scaled(u64),
    /// A type, or an option, this reader does not search yet.
    Unsupported(String),
}

/// One field: its type and whether it is indexed and has doc values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The type.
    pub ty: FieldType,
    /// The pin a `text` field is analysed with, when the index pins one
    /// ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md));
    /// none for the standard analyzer.
    pub analyzer: Option<String>,
    /// `index`, true by default.
    pub indexed: bool,
    /// `doc_values`, true by default.
    pub doc_values: bool,
    /// A `knn_vector` field's `dimension` and space; its type is unsupported
    /// for every query but `knn`.
    pub knn: Option<KnnField>,
    /// What the `fields` API reads of the field.
    pub fetch: Fetch,
}

/// What the `fields` API needs of a field: where its values sit in `_source`
/// and the options that change what is returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fetch {
    /// The full name of the field a multi-field is read from; none for a
    /// field that reads its own path.
    pub parent: Option<String>,
    /// `ignore_above`: a keyword longer than this many UTF-16 units is not returned.
    pub ignore_above: Option<i64>,
    /// An option the `fields` API does not serve (`null_value`, `ignore_malformed`).
    pub unserved: Option<&'static str>,
    /// `store: true`: the field's value is also kept as a stored field.
    pub stored: bool,
}

pub use crate::mapping_types::{KnnField, Space};

/// An index's fields by full name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapping {
    fields: BTreeMap<String, Field>,
    /// The full paths of the `nested` objects.
    nested: BTreeSet<String>,
    /// The fields some `copy_to` names, whose source paths include the copying fields'.
    copy_targets: BTreeSet<String>,
    /// The full paths of the object fields (those with sub-fields and no type), nested ones included.
    objects: BTreeSet<String>,
}

/// Why a mapping could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingError(pub String);

impl std::fmt::Display for MappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mapping: {}", self.0)
    }
}

impl From<JsonError> for MappingError {
    fn from(e: JsonError) -> Self {
        // ALLOC: error path; one message.
        MappingError(e.to_string())
    }
}

impl From<crate::dsl::DslError> for MappingError {
    fn from(e: crate::dsl::DslError) -> Self {
        MappingError(e.into_failure().reason)
    }
}

/// Collects a JSON string's decoded pieces.
struct Unescape {
    text: String,
}

impl TokenSink for Unescape {
    type Error = MappingError;

    fn token(&mut self, token: Token<'_>) -> Result<(), MappingError> {
        match token {
            Token::Str { text, .. }
                if self.text.len().saturating_add(text.len()) <= MAX_MAPPING_BYTES =>
            {
                self.text.push_str(text);
                Ok(())
            }
            Token::Str { .. } => Err(MappingError("larger than its limit".into())),
            _ => Err(MappingError("not a string".into())),
        }
    }

    fn token_at(&mut self, token: Token<'_>, _: Location) -> Result<(), MappingError> {
        self.token(token)
    }
}

impl Mapping {
    /// Reads the mapping from its escaped form in a descriptor.
    pub fn from_escaped(escaped: &[u8]) -> Result<Self, MappingError> {
        Self::from_json(&Self::unescape(escaped)?)
    }

    /// The JSON source of a mapping in its escaped form in a descriptor.
    pub fn unescape(escaped: &[u8]) -> Result<String, MappingError> {
        let mut tokenizer = Tokenizer::new(Limits::default());
        let mut sink = Unescape {
            // ALLOC: the decoded mapping, at most MAX_MAPPING_BYTES; once per index opened.
            text: String::with_capacity(escaped.len()),
        };
        tokenizer.feed(b"\"", &mut sink)?;
        tokenizer.feed(escaped, &mut sink)?;
        tokenizer.feed(b"\"", &mut sink)?;
        tokenizer.finish(&mut sink)?;
        Ok(sink.text)
    }

    /// Reads a mapping's JSON source.
    pub fn from_json(json: &str) -> Result<Self, MappingError> {
        Self::from_json_pinned(json, &[])
    }

    /// Reads a mapping's JSON source for an index whose settings pin the
    /// analyzers `pins`: a `text` field that names one is searched with it,
    /// and any other analyzer leaves the field unsupported.
    pub fn from_json_pinned(json: &str, pins: &[String]) -> Result<Self, MappingError> {
        let mut rec = Recorder::new(MAX_MAPPING_BYTES);
        let mut tokenizer = Tokenizer::new(Limits::default());
        tokenizer.feed(json.as_bytes(), &mut rec)?;
        tokenizer.finish(&mut rec)?;
        let mut mapping = Mapping::default();
        let mut p = rec.parser();
        if p.next_token() != Some(Kind::StartObject) {
            return Err(MappingError("not an object".into()));
        }
        // `{"_doc": {...}}`, the typed form, holds the mapping one level down.
        let mut probe = p.clone();
        if probe.next_token() == Some(Kind::FieldName) && probe.text() == "_doc" {
            p.next_token();
            p.next_token();
        }
        mapping.object(&mut p, "", pins)?;
        Ok(mapping)
    }

    /// The field named `name`.
    pub fn get(&self, name: &str) -> Option<&Field> {
        self.fields.get(name)
    }

    /// Whether `path` is an object field, `nested` or not.
    pub fn is_object(&self, path: &str) -> bool {
        self.objects.contains(path)
    }

    /// Whether a `copy_to` names the field `name`.
    pub fn is_copy_target(&self, name: &str) -> bool {
        self.copy_targets.contains(name)
    }

    /// Whether `path` is a `nested` object.
    pub fn is_nested(&self, path: &str) -> bool {
        self.nested.contains(path)
    }

    /// Whether the index has any `nested` object, which makes every search
    /// skip the nested documents.
    pub fn has_nested(&self) -> bool {
        !self.nested.is_empty()
    }

    /// The nearest `nested` object that holds the nested object `path`
    /// (`ObjectMapper.nested().getParentObjectMapper`), none for a top-level one.
    pub fn nested_parent(&self, path: &str) -> Option<&str> {
        self.nested
            .iter()
            .filter(|n| {
                path.strip_prefix(n.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
            })
            .max_by_key(|n| n.len())
            .map(String::as_str)
    }

    /// Every field's name, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.fields.keys().map(String::as_str)
    }

    /// How many fields there are.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The body of a mapping or object field: on its start, reads to its end.
    fn object(
        &mut self,
        p: &mut Parser<'_>,
        prefix: &str,
        pins: &[String],
    ) -> Result<(), MappingError> {
        while let Some(Kind::FieldName) = p.next_token() {
            let key = p.text();
            p.next_token();
            if key == "properties" && p.current_token() == Some(Kind::StartObject) {
                self.properties(p, (prefix, None), pins)?;
            } else {
                p.skip_children();
            }
        }
        Ok(())
    }

    /// `properties`: on its start, reads to its end.
    fn properties(
        &mut self,
        p: &mut Parser<'_>,
        (prefix, multi_of): (&str, Option<&str>),
        pins: &[String],
    ) -> Result<(), MappingError> {
        while let Some(Kind::FieldName) = p.next_token() {
            // ALLOC: one full name a field, at most MAX_FIELDS.
            let name = format!("{prefix}{}", p.text());
            if p.next_token() != Some(Kind::StartObject) {
                // ALLOC: error path; one message.
                return Err(MappingError(format!("field [{name}] is not an object")));
            }
            self.field(p, (&name, multi_of), pins)?;
        }
        Ok(())
    }

    /// A `copy_to`'s targets: on its value, reads to its end.
    fn copy_to(&mut self, p: &mut Parser<'_>) {
        match p.current_token() {
            // ALLOC: one name a target, bounded by the recorded mapping.
            Some(Kind::String) => {
                self.copy_targets.insert(p.text().to_owned());
            }
            Some(Kind::StartArray) => {
                while p.next_token() == Some(Kind::String) {
                    // ALLOC: as above.
                    self.copy_targets.insert(p.text().to_owned());
                }
            }
            _ => {}
        }
    }

    /// One field's definition: on its start, reads to its end.
    fn field(
        &mut self,
        p: &mut Parser<'_>,
        (name, multi_of): (&str, Option<&str>),
        pins: &[String],
    ) -> Result<(), MappingError> {
        let mut fetch = Fetch::default();
        let mut ty = None;
        let mut indexed = true;
        let mut doc_values = true;
        let mut options = None;
        let mut dimension = None;
        let mut scaling = None;
        let mut space = None;
        let (mut analyzer, mut search, mut quote) = (None, None, None);
        while let Some(Kind::FieldName) = p.next_token() {
            let key = p.text();
            p.next_token();
            let text = p.text();
            match key {
                "type" => ty = Some(text),
                "index" => indexed = text != "false",
                "doc_values" => doc_values = text != "false",
                "dimension" => dimension = text.parse::<u32>().ok(),
                "scaling_factor" => scaling = text.parse::<f64>().ok(),
                "space_type" => space = Some(text),
                "method" if p.current_token() == Some(Kind::StartObject) => {
                    while let Some(Kind::FieldName) = p.next_token() {
                        let inner = p.text();
                        p.next_token();
                        if inner == "space_type" {
                            space = Some(p.text());
                        }
                        p.skip_children();
                    }
                }
                "analyzer" => analyzer = Some(text),
                "search_analyzer" => search = Some(text),
                "search_quote_analyzer" => quote = Some(text),
                "normalizer" | "format" => {
                    options = Some(key);
                }
                "ignore_above" => fetch.ignore_above = text.parse().ok(),
                "null_value" => fetch.unserved = Some("null_value"),
                "store" if text == "true" => fetch.stored = true,
                "ignore_malformed" if text == "true" => fetch.unserved = Some("ignore_malformed"),
                "coerce" if text == "false" => fetch.unserved = Some("coerce"),
                "index_prefixes" => {
                    fetch.unserved = Some("index_prefixes");
                    p.skip_children();
                }
                "index_phrases" if text == "true" => fetch.unserved = Some("index_phrases"),
                "copy_to" => self.copy_to(p),
                "properties" => {
                    // ALLOC: one prefix an object field, at most MAX_FIELDS.
                    let prefix = format!("{name}.");
                    self.properties(p, (&prefix, None), pins)?;
                }
                "fields" => {
                    // ALLOC: one prefix a field with multi-fields, at most MAX_FIELDS.
                    let prefix = format!("{name}.");
                    self.properties(p, (&prefix, Some(name)), pins)?;
                }
                _ => p.skip_children(),
            }
        }
        if ty == Some("nested") {
            if self.nested.len() >= MAX_FIELDS {
                // ALLOC: error path; one message.
                return Err(MappingError(format!("more than [{MAX_FIELDS}] fields")));
            }
            // ALLOC: one path a nested object, at most MAX_FIELDS.
            self.nested.insert(name.to_owned());
            // ALLOC: one path a nested object, at most MAX_FIELDS.
            self.objects.insert(name.to_owned());
            return Ok(());
        }
        let Some(ty) = ty else {
            // An object field: its sub-fields are already in.
            if self.objects.len() < MAX_FIELDS {
                // ALLOC: one path an object, at most MAX_FIELDS.
                self.objects.insert(name.to_owned());
            }
            return Ok(());
        };
        // A pin the index holds, named by the analyzer and by no other
        // search analyzer, is how the field is searched.
        let pinned = analyzer
            .filter(|a| ty == "text" && pins.iter().any(|p| p == a))
            .filter(|a| search.is_none_or(|s| s == *a) && quote.is_none_or(|q| q == *a));
        if pinned.is_none() {
            if analyzer.is_some_and(|a| a != "standard") {
                options = options.or(Some("analyzer"));
            }
            if search.is_some_and(|a| a != "standard") {
                options = options.or(Some("search_analyzer"));
            }
            if quote.is_some() {
                options = options.or(Some("search_quote_analyzer"));
            }
        }
        let is_knn = ty == "knn_vector";
        let ty = match (ty, options) {
            (other, Some(option)) => {
                // ALLOC: one description a field this reader does not search.
                FieldType::Unsupported(format!("a [{other}] field with [{option}]"))
            }
            ("text", None) => FieldType::Text,
            ("keyword", None) => FieldType::Keyword,
            ("byte", None) => FieldType::Int {
                name: "byte",
                min: i64::from(i8::MIN),
                max: i64::from(i8::MAX),
            },
            ("short", None) => FieldType::Int {
                name: "short",
                min: i64::from(i16::MIN),
                max: i64::from(i16::MAX),
            },
            ("integer", None) => FieldType::Int {
                name: "integer",
                min: i64::from(i32::MIN),
                max: i64::from(i32::MAX),
            },
            ("long", None) => FieldType::Long,
            ("date", None) => FieldType::Date,
            ("float", None) => FieldType::Float,
            ("double", None) => FieldType::Double,
            ("half_float", None) => FieldType::HalfFloat,
            ("scaled_float", None) if scaling.is_some_and(|f| f.is_finite() && f > 0.0) => {
                FieldType::Scaled(scaling.unwrap_or(1.0).to_bits())
            }
            // ALLOC: one description a field this reader does not search.
            (other, None) => FieldType::Unsupported(format!("a [{other}] field")),
        };
        if self.fields.len() >= MAX_FIELDS {
            // ALLOC: error path; one message.
            return Err(MappingError(format!("more than [{MAX_FIELDS}] fields")));
        }
        self.fields.insert(
            // ALLOC: one name a field, at most MAX_FIELDS.
            name.to_owned(),
            Field {
                ty,
                // ALLOC: one name a pinned field, at most MAX_FIELDS.
                analyzer: pinned.map(str::to_owned),
                indexed,
                doc_values,
                fetch: Fetch {
                    // ALLOC: one name a multi-field, at most MAX_FIELDS.
                    parent: multi_of.map(str::to_owned),
                    ..fetch
                },
                knn: dimension.filter(|_| is_knn).map(|dimension| KnnField {
                    dimension,
                    space: match space {
                        Some("cosinesimil") => Space::CosineSimil,
                        Some("innerproduct") => Space::InnerProduct,
                        _ => Space::L2,
                    },
                }),
            },
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
