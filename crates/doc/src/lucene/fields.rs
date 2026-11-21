//! What each mapped field puts in a Lucene document, read from its mapping
//! once a segment: the shape of its values, and the `FieldInfo` the
//! engine registers it with, as each field mapper's Lucene field types
//! give it.

use lucene_codecs::field_infos::{DocValuesSkipIndexType, DocValuesType, FieldInfo, IndexOptions};

use crate::mapping::{FieldType, Leaf, NumType, Value};

/// `TextFieldMapper.Defaults.POSITION_INCREMENT_GAP`, used when the
/// mapping leaves the analyzer's gap.
pub const POSITION_INCREMENT_GAP: i32 = 100;

/// `DataStreamFieldMapper`'s timestamp field, which a date gets a
/// doc-values skip index on (`isSkiplistDefaultEnabled`).
const TIMESTAMP_FIELD: &str = "@timestamp";

/// How a field's values become Lucene fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Analyzed text, with its position increment gap.
    Text {
        /// The gap between two values' positions.
        gap: i32,
    },
    /// One term a value.
    Keyword,
    /// A 4-byte point: byte, short, integer.
    Int,
    /// An 8-byte point: long.
    Long,
    /// A float, sortable as an int.
    Float,
    /// A double, sortable as a long.
    Double,
    /// Milliseconds as a long.
    Date,
    /// `T` or `F`.
    Bool,
}

/// One mapped field as the engine sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// How values become Lucene fields.
    pub shape: Shape,
    /// What its postings keep; `None` when it is not indexed as terms.
    pub index_options: IndexOptions,
    /// Whether it has points.
    pub points: bool,
    /// Whether it keeps norms.
    pub norms: bool,
    /// Whether it has doc values.
    pub doc_values: bool,
    /// Whether its values are stored.
    pub store: bool,
}

fn flag(leaf: &Leaf, name: &str) -> bool {
    leaf.get(name).is_some_and(Value::flag)
}

/// A keyword's `index_options`: `docs`, or `freqs` (`TextParams.toIndexOptions`).
fn keyword_options(leaf: &Leaf) -> IndexOptions {
    match leaf.get("index_options").and_then(Value::text) {
        Some("freqs") => IndexOptions::DocsAndFreqs,
        _ => IndexOptions::Docs,
    }
}

fn text_options(leaf: &Leaf) -> IndexOptions {
    match leaf.get("index_options").and_then(Value::text) {
        Some("docs") => IndexOptions::Docs,
        Some("freqs") => IndexOptions::DocsAndFreqs,
        Some("offsets") => IndexOptions::DocsAndFreqsAndPositionsAndOffsets,
        _ => IndexOptions::DocsAndFreqsAndPositions,
    }
}

impl Spec {
    /// The field `leaf` maps.
    pub fn of(leaf: &Leaf) -> Self {
        let index = flag(leaf, "index");
        let store = flag(leaf, "store");
        let doc_values = !matches!(leaf.ty(), FieldType::Text) && flag(leaf, "doc_values");
        let (shape, index_options, points, norms) = match leaf.ty() {
            FieldType::Text => {
                let gap = match leaf.get("position_increment_gap") {
                    Some(Value::Int(g)) if *g >= 0 => *g,
                    _ => POSITION_INCREMENT_GAP,
                };
                (
                    Shape::Text { gap },
                    if index {
                        text_options(leaf)
                    } else {
                        IndexOptions::None
                    },
                    false,
                    flag(leaf, "norms"),
                )
            }
            FieldType::Keyword => (
                Shape::Keyword,
                if index {
                    keyword_options(leaf)
                } else {
                    IndexOptions::None
                },
                false,
                flag(leaf, "norms"),
            ),
            FieldType::Boolean => (
                Shape::Bool,
                if index {
                    IndexOptions::Docs
                } else {
                    IndexOptions::None
                },
                false,
                false,
            ),
            FieldType::Date => (Shape::Date, IndexOptions::None, index, false),
            FieldType::Number(n) => {
                let shape = match n {
                    NumType::Byte | NumType::Short | NumType::Integer => Shape::Int,
                    NumType::Long => Shape::Long,
                    NumType::Float => Shape::Float,
                    NumType::Double => Shape::Double,
                };
                (shape, IndexOptions::None, index, false)
            }
        };
        Spec {
            shape,
            index_options,
            points,
            norms,
            doc_values,
            store,
        }
    }

    /// Whether the field's terms are indexed.
    pub fn inverted(&self) -> bool {
        self.index_options != IndexOptions::None
    }

    /// Whether the mapper adds `_field_names` for a value
    /// (`createFieldNamesField`'s callers): text without norms; a keyword
    /// without doc values or norms; a number, date or boolean without doc
    /// values.
    pub fn field_names(&self) -> bool {
        match self.shape {
            Shape::Text { .. } => (self.inverted() || self.store) && !self.norms,
            Shape::Keyword => (self.inverted() || self.store) && !self.doc_values && !self.norms,
            Shape::Bool => !self.doc_values && (self.inverted() || self.store),
            _ => !self.doc_values && (self.points || self.store),
        }
    }

    /// Whether a value adds anything at all.
    pub fn adds_anything(&self) -> bool {
        self.inverted() || self.points || self.doc_values || self.store
    }

    /// The `FieldInfo` the engine registers the field `name` with as
    /// `number`. `IndexingChain` records norms only for an indexed field, so
    /// a field without postings never omits them.
    pub fn field_info(&self, name: &str, number: i32) -> FieldInfo {
        let mut info = FieldInfo::new(name, number)
            .with_index_options(self.index_options)
            .with_omit_norms(self.inverted() && !self.norms);
        if self.doc_values {
            let ty = if self.shape == Shape::Keyword {
                DocValuesType::SortedSet
            } else {
                DocValuesType::SortedNumeric
            };
            let skip = if self.shape == Shape::Date && name == TIMESTAMP_FIELD {
                DocValuesSkipIndexType::Range
            } else {
                DocValuesSkipIndexType::None
            };
            info = info.with_doc_values(ty, skip, -1);
        }
        if self.points {
            let bytes = match self.shape {
                Shape::Int | Shape::Float => 4,
                _ => 8,
            };
            info = info.with_points(1, 1, bytes);
        }
        info
    }
}

/// The metadata fields, in the order a document adds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaField {
    /// `_id`: one term and its stored bytes.
    Id,
    /// `_routing`: one term and its stored string, only on a document
    /// indexed with a custom routing value.
    Routing,
    /// `_seq_no`: a long point and numeric doc values.
    SeqNo,
    /// `_primary_term`: numeric doc values.
    PrimaryTerm,
    /// `_source`: stored bytes.
    Source,
    /// `_version`: numeric doc values.
    Version,
    /// `_field_names`: one term a field path.
    FieldNames,
    /// `_ignored`: one term and stored string a field.
    Ignored,
    /// `_tombstone`: numeric doc values 1 on a delete's tombstone.
    Tombstone,
    /// `__soft_deletes`: numeric doc values 1 on a replaced or deleted
    /// document, the engine's soft-deletes field.
    SoftDeletes,
}

impl MetaField {
    /// The field's name.
    pub fn name(self) -> &'static str {
        match self {
            MetaField::Id => "_id",
            MetaField::Routing => "_routing",
            MetaField::SeqNo => "_seq_no",
            MetaField::PrimaryTerm => "_primary_term",
            MetaField::Source => "_source",
            MetaField::Version => "_version",
            MetaField::FieldNames => "_field_names",
            MetaField::Ignored => "_ignored",
            MetaField::Tombstone => "_tombstone",
            MetaField::SoftDeletes => "__soft_deletes",
        }
    }

    /// The `FieldInfo` the engine registers it with as `number`.
    pub fn field_info(self, number: i32) -> FieldInfo {
        let info = FieldInfo::new(self.name(), number);
        let numeric = |i: FieldInfo| {
            i.with_doc_values(DocValuesType::Numeric, DocValuesSkipIndexType::None, -1)
        };
        match self {
            MetaField::Id | MetaField::Routing | MetaField::FieldNames | MetaField::Ignored => info
                .with_index_options(IndexOptions::Docs)
                .with_omit_norms(true),
            MetaField::SeqNo => numeric(info).with_points(1, 1, 8),
            MetaField::PrimaryTerm | MetaField::Version | MetaField::Tombstone => numeric(info),
            MetaField::SoftDeletes => numeric(info).with_soft_deletes_field(true),
            MetaField::Source => info,
        }
    }
}
