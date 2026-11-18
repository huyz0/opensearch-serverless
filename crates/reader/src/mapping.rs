//! An index's field types, read from the mapping in its descriptor.
//!
//! The shell keeps an index's mapping as a JSON string inside its
//! descriptor (`IndexDescriptor.toXContent`'s `mapping`). This decodes that
//! string with `oss_rest`'s tokenizer and reads each field's `type` and the
//! options that change how it is searched, walking `properties` of objects
//! and multi-fields (`fields`) into dotted names, as `MapperService` names
//! them. Only what a query of the subset needs is kept.

use std::collections::BTreeMap;

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
    /// A type, or an option, this reader does not search yet.
    Unsupported(String),
}

/// One field: its type and whether it is indexed and has doc values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The type.
    pub ty: FieldType,
    /// `index`, true by default.
    pub indexed: bool,
    /// `doc_values`, true by default.
    pub doc_values: bool,
}

/// An index's fields by full name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapping {
    fields: BTreeMap<String, Field>,
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
        mapping.object(&mut p, "")?;
        Ok(mapping)
    }

    /// The field named `name`.
    pub fn get(&self, name: &str) -> Option<&Field> {
        self.fields.get(name)
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
    fn object(&mut self, p: &mut Parser<'_>, prefix: &str) -> Result<(), MappingError> {
        while let Some(Kind::FieldName) = p.next_token() {
            let key = p.text();
            p.next_token();
            if key == "properties" && p.current_token() == Some(Kind::StartObject) {
                self.properties(p, prefix)?;
            } else {
                p.skip_children();
            }
        }
        Ok(())
    }

    /// `properties`: on its start, reads to its end.
    fn properties(&mut self, p: &mut Parser<'_>, prefix: &str) -> Result<(), MappingError> {
        while let Some(Kind::FieldName) = p.next_token() {
            // ALLOC: one full name a field, at most MAX_FIELDS.
            let name = format!("{prefix}{}", p.text());
            if p.next_token() != Some(Kind::StartObject) {
                // ALLOC: error path; one message.
                return Err(MappingError(format!("field [{name}] is not an object")));
            }
            self.field(p, &name)?;
        }
        Ok(())
    }

    /// One field's definition: on its start, reads to its end.
    fn field(&mut self, p: &mut Parser<'_>, name: &str) -> Result<(), MappingError> {
        let mut ty = None;
        let mut indexed = true;
        let mut doc_values = true;
        let mut options = None;
        while let Some(Kind::FieldName) = p.next_token() {
            let key = p.text();
            p.next_token();
            let text = p.text();
            match key {
                "type" => ty = Some(text),
                "index" => indexed = text != "false",
                "doc_values" => doc_values = text != "false",
                "analyzer" | "search_analyzer" if text == "standard" => {}
                "analyzer"
                | "search_analyzer"
                | "normalizer"
                | "format"
                | "search_quote_analyzer" => {
                    options = Some(key);
                }
                "properties" => {
                    // ALLOC: one prefix an object field, at most MAX_FIELDS.
                    let prefix = format!("{name}.");
                    self.properties(p, &prefix)?;
                }
                "fields" => {
                    // ALLOC: one prefix a field with multi-fields, at most MAX_FIELDS.
                    let prefix = format!("{name}.");
                    self.properties(p, &prefix)?;
                }
                _ => p.skip_children(),
            }
        }
        let Some(ty) = ty else {
            // An object field: its sub-fields are already in.
            return Ok(());
        };
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
                indexed,
                doc_values,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(m: &Mapping, name: &str) -> Option<FieldType> {
        m.get(name).map(|f| f.ty.clone())
    }

    #[test]
    fn types_objects_and_multi_fields() {
        let m = Mapping::from_json(
            r#"{"dynamic":"strict","properties":{"title":{"type":"text","fields":{"raw":{"type":"keyword"}}},
            "year":{"type":"integer"},"n":{"type":"long","index":false},"b":{"type":"byte"},"s":{"type":"short","doc_values":false},
            "when":{"type":"date"},"user":{"properties":{"name":{"type":"keyword"},"meta":{"enabled":false}}},
            "geo":{"type":"geo_point"},"en":{"type":"text","analyzer":"english"},"std":{"type":"text","analyzer":"standard"},
            "lower":{"type":"keyword","normalizer":"lowercase"},"d2":{"type":"date","format":"yyyy"}}}"#,
        )
        .expect("parses");
        assert_eq!(ty(&m, "title"), Some(FieldType::Text));
        assert_eq!(ty(&m, "title.raw"), Some(FieldType::Keyword));
        assert_eq!(ty(&m, "std"), Some(FieldType::Text));
        assert_eq!(
            ty(&m, "year").map(|t| matches!(
                t,
                FieldType::Int {
                    name: "integer",
                    ..
                }
            )),
            Some(true)
        );
        assert_eq!(
            ty(&m, "b").map(|t| matches!(
                t,
                FieldType::Int {
                    min: -128,
                    max: 127,
                    ..
                }
            )),
            Some(true)
        );
        assert_eq!(
            ty(&m, "s").map(|t| matches!(t, FieldType::Int { name: "short", .. })),
            Some(true)
        );
        assert_eq!(m.get("s").map(|f| f.doc_values), Some(false));
        assert_eq!(ty(&m, "n"), Some(FieldType::Long));
        assert_eq!(m.get("n").map(|f| f.indexed), Some(false));
        assert_eq!(ty(&m, "when"), Some(FieldType::Date));
        assert_eq!(ty(&m, "user.name"), Some(FieldType::Keyword));
        assert_eq!(ty(&m, "user"), None, "an object is not a field");
        assert_eq!(
            ty(&m, "geo"),
            Some(FieldType::Unsupported("a [geo_point] field".into()))
        );
        assert_eq!(
            ty(&m, "en"),
            Some(FieldType::Unsupported(
                "a [text] field with [analyzer]".into()
            ))
        );
        assert!(matches!(ty(&m, "lower"), Some(FieldType::Unsupported(_))));
        assert!(matches!(ty(&m, "d2"), Some(FieldType::Unsupported(_))));
        assert_eq!(m.len(), 13);
        assert!(!m.is_empty());
    }

    #[test]
    fn the_escaped_form_and_the_typed_form() {
        let m =
            Mapping::from_escaped(br#"{\"_doc\":{\"properties\":{\"t\":{\"type\":\"text\"}}}}"#)
                .expect("parses");
        assert_eq!(ty(&m, "t"), Some(FieldType::Text));
        assert!(Mapping::from_json("{}").expect("empty").is_empty());
    }

    #[test]
    fn malformed_mappings_are_refused() {
        assert!(Mapping::from_json("[]").is_err());
        assert!(Mapping::from_json(r#"{"properties":{"a":1}}"#).is_err());
        assert!(Mapping::from_json("{").is_err());
        assert!(Mapping::from_escaped(br#"\q"#).is_err());
        let many: Vec<String> = (0..=MAX_FIELDS)
            .map(|i| format!(r#""f{i}":{{"type":"long"}}"#))
            .collect();
        let json = format!(r#"{{"properties":{{{}}}}}"#, many.join(","));
        assert_eq!(
            Mapping::from_json(&json),
            Err(MappingError(format!("more than [{MAX_FIELDS}] fields")))
        );
        let at_limit = format!(r#"{{"properties":{{{}}}}}"#, many[..MAX_FIELDS].join(","));
        assert_eq!(
            Mapping::from_json(&at_limit).map(|m| m.len()),
            Ok(MAX_FIELDS)
        );
        assert_eq!(MappingError("x".into()).to_string(), "mapping: x");
    }
}
