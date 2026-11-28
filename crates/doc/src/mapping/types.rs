//! The leaf field types this writer indexes and their parameter tables.
//!
//! Ported from the `getParameters` lists of `TextFieldMapper`,
//! `KeywordFieldMapper`, `NumberFieldMapper`, `DateFieldMapper` and
//! `BooleanFieldMapper` (server, OpenSearch 3.10), in the Java's order,
//! which is the order they are written in; `text` keeps its own
//! backwards-compatible order ([`TEXT_ORDER`]).

use super::numbers::NumType;
use super::params::{Def, Default as D, Kind, Merge};

/// A leaf field's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FieldType {
    /// `text`
    Text,
    /// `keyword`
    Keyword,
    /// A numeric type.
    Number(NumType),
    /// `date`
    Date,
    /// `boolean`
    Boolean,
    /// `knn_vector`, the k-NN plugin's field.
    Knn,
    /// `geo_point`: a latitude and longitude, as `LatLonPoint`s and doc values.
    GeoPoint,
}

impl FieldType {
    /// The type for a mapping's `type`, if this writer knows it.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "text" => FieldType::Text,
            "keyword" => FieldType::Keyword,
            "byte" => FieldType::Number(NumType::Byte),
            "short" => FieldType::Number(NumType::Short),
            "integer" => FieldType::Number(NumType::Integer),
            "long" => FieldType::Number(NumType::Long),
            "float" => FieldType::Number(NumType::Float),
            "double" => FieldType::Number(NumType::Double),
            "half_float" => FieldType::Number(NumType::HalfFloat),
            "scaled_float" => FieldType::Number(NumType::ScaledFloat),
            "date" => FieldType::Date,
            "boolean" => FieldType::Boolean,
            "knn_vector" => FieldType::Knn,
            "geo_point" => FieldType::GeoPoint,
            _ => return None,
        })
    }

    /// The type's name.
    pub fn name(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Keyword => "keyword",
            FieldType::Number(n) => n.name(),
            FieldType::Date => "date",
            FieldType::Boolean => "boolean",
            FieldType::Knn => "knn_vector",
            FieldType::GeoPoint => "geo_point",
        }
    }

    /// The type's parameters, in `getParameters` order.
    pub fn defs(self) -> &'static [Def] {
        match self {
            FieldType::Text => TEXT,
            FieldType::Keyword => KEYWORD,
            FieldType::Number(NumType::ScaledFloat) => SCALED,
            FieldType::Number(_) => NUMBER,
            FieldType::Date => DATE,
            FieldType::Boolean => BOOLEAN,
            FieldType::Knn => KNN,
            FieldType::GeoPoint => GEO_POINT,
        }
    }
}

/// Types OpenSearch knows that this writer does not index yet.
pub(crate) const OTHER_TYPES: &[&str] = &[
    "nested",
    "alias",
    "binary",
    "unsigned_long",
    "date_nanos",
    "ip",
    "geo_shape",
    "completion",
    "search_as_you_type",
    "token_count",
    "flat_object",
    "wildcard",
    "constant_keyword",
    "match_only_text",
    "integer_range",
    "long_range",
    "float_range",
    "double_range",
    "date_range",
    "ip_range",
    "rank_feature",
    "rank_features",
    "percolator",
    "join",
    "derived",
    "star_tree",
    "semantic",
];

const fn p(name: &'static str, kind: Kind, merge: Merge, default: D) -> Def {
    Def {
        name,
        kind,
        merge,
        default,
        only_default: false,
    }
}

const fn only(name: &'static str, kind: Kind, merge: Merge, default: D) -> Def {
    Def {
        name,
        kind,
        merge,
        default,
        only_default: true,
    }
}

use Merge::{Fixed, Norms, Updateable as Upd};

pub(crate) const TEXT: &[Def] = &[
    p("index", Kind::Bool, Fixed, D::Bool(true)),
    p("store", Kind::Bool, Fixed, D::Bool(false)),
    p(
        "index_options",
        Kind::Choice(&["positions", "docs", "freqs", "offsets"]),
        Fixed,
        D::Str("positions"),
    ),
    p("norms", Kind::Bool, Norms, D::Bool(true)),
    only(
        "term_vector",
        Kind::Choice(&[
            "no",
            "yes",
            "with_positions",
            "with_offsets",
            "with_positions_offsets",
            "with_positions_payloads",
            "with_positions_offsets_payloads",
        ]),
        Fixed,
        D::Str("no"),
    ),
    p("analyzer", Kind::Analyzer, Fixed, D::Str("default")),
    p(
        "search_analyzer",
        Kind::Analyzer,
        Upd,
        D::Same(TEXT_ANALYZER),
    ),
    p(
        "search_quote_analyzer",
        Kind::Analyzer,
        Upd,
        D::Same(TEXT_ANALYZER + 1),
    ),
    only("similarity", Kind::NullString, Fixed, D::Null),
    p(
        "position_increment_gap",
        Kind::Int,
        Fixed,
        D::Int(POSITION_INCREMENT_GAP_USE_ANALYZER),
    ),
    p("fielddata", Kind::Bool, Upd, D::Bool(false)),
    only("fielddata_frequency_filter", Kind::NullString, Upd, D::Null),
    p("eager_global_ordinals", Kind::Bool, Upd, D::Bool(false)),
    only("index_phrases", Kind::Bool, Fixed, D::Bool(false)),
    only("index_prefixes", Kind::NullString, Fixed, D::Null),
    p("boost", Kind::Boost, Upd, D::Float(1.0)),
    p("meta", Kind::Meta, Upd, D::Empty),
];

/// Where `analyzer` sits in [`TEXT`].
const TEXT_ANALYZER: usize = 5;

/// `TextFieldMapper.POSITION_INCREMENT_GAP_USE_ANALYZER`.
pub const POSITION_INCREMENT_GAP_USE_ANALYZER: i32 = -1;

/// `TextFieldMapper.doXContentBody`'s order, kept for backwards
/// compatibility; `fields` stands for the multi-fields.
pub(crate) const TEXT_ORDER: &[&str] = &[
    "boost",
    "index",
    "store",
    "fields",
    "meta",
    "index_options",
    "term_vector",
    "norms",
    "analyzer",
    "search_analyzer",
    "search_quote_analyzer",
    "similarity",
    "eager_global_ordinals",
    "position_increment_gap",
    "fielddata",
    "fielddata_frequency_filter",
    "index_prefixes",
    "index_phrases",
];

const KEYWORD: &[Def] = &[
    p("index", Kind::Bool, Fixed, D::Bool(true)),
    p("doc_values", Kind::Bool, Fixed, D::Bool(true)),
    p("store", Kind::Bool, Fixed, D::Bool(false)),
    p("null_value", Kind::NullString, Fixed, D::Null),
    p("eager_global_ordinals", Kind::Bool, Upd, D::Bool(false)),
    p("ignore_above", Kind::Int, Upd, D::Int(i32::MAX)),
    p(
        "index_options",
        Kind::Choice(&["docs", "freqs"]),
        Fixed,
        D::Str("docs"),
    ),
    p("norms", Kind::Bool, Norms, D::Bool(false)),
    only("similarity", Kind::NullString, Fixed, D::Null),
    only("use_similarity", Kind::Bool, Upd, D::Bool(false)),
    only("normalizer", Kind::Str, Fixed, D::Str("default")),
    p(
        "split_queries_on_whitespace",
        Kind::Bool,
        Upd,
        D::Bool(false),
    ),
    p("boost", Kind::Boost, Upd, D::Float(1.0)),
    p("meta", Kind::Meta, Upd, D::Empty),
    only("multi_value", Kind::Bool, Upd, D::Null),
];

const NUMBER: &[Def] = &[
    p("index", Kind::Bool, Fixed, D::Bool(true)),
    p("doc_values", Kind::Bool, Fixed, D::Bool(true)),
    p("store", Kind::Bool, Fixed, D::Bool(false)),
    only("skip_list", Kind::Bool, Fixed, D::Bool(false)),
    p("ignore_malformed", Kind::Explicit, Upd, D::Bool(false)),
    p("coerce", Kind::Explicit, Upd, D::Bool(true)),
    p("null_value", Kind::NullNumber, Fixed, D::Null),
    p("meta", Kind::Meta, Upd, D::Empty),
];

/// `ScaledFloatFieldMapper.Builder.getParameters`, in its order.
const SCALED: &[Def] = &[
    p("index", Kind::Bool, Fixed, D::Bool(true)),
    p("doc_values", Kind::Bool, Fixed, D::Bool(true)),
    p("store", Kind::Bool, Fixed, D::Bool(false)),
    p("ignore_malformed", Kind::Explicit, Upd, D::Bool(false)),
    p("meta", Kind::Meta, Upd, D::Empty),
    p("scaling_factor", Kind::ScalingFactor, Fixed, D::Null),
    p("coerce", Kind::Explicit, Upd, D::Bool(true)),
    p("null_value", Kind::NullNumber, Fixed, D::Null),
    only("skip_list", Kind::Bool, Fixed, D::Bool(false)),
];

/// `DateFieldMapper.DEFAULT_DATE_TIME_FORMATTER`'s pattern.
pub const DEFAULT_DATE_FORMAT: &str = "strict_date_optional_time||epoch_millis";

const DATE: &[Def] = &[
    p("index", Kind::Bool, Fixed, D::Bool(true)),
    p("doc_values", Kind::Bool, Fixed, D::Bool(true)),
    p("store", Kind::Bool, Fixed, D::Bool(false)),
    only("skip_list", Kind::Bool, Fixed, D::Bool(false)),
    p("format", Kind::Str, Fixed, D::Str(DEFAULT_DATE_FORMAT)),
    p(
        "print_format",
        Kind::NullString,
        Fixed,
        D::Str("strict_date_optional_time"),
    ),
    only("locale", Kind::Str, Fixed, D::Str("")),
    p("null_value", Kind::NullString, Fixed, D::Null),
    p("ignore_malformed", Kind::Explicit, Upd, D::Bool(false)),
    p("boost", Kind::Boost, Upd, D::Float(1.0)),
    p("meta", Kind::Meta, Upd, D::Empty),
];

const BOOLEAN: &[Def] = &[
    p("meta", Kind::Meta, Upd, D::Empty),
    p("boost", Kind::Boost, Upd, D::Float(1.0)),
    p("doc_values", Kind::Bool, Fixed, D::Bool(true)),
    p("index", Kind::Bool, Fixed, D::Bool(true)),
    p("null_value", Kind::NullBool, Fixed, D::Null),
    p("store", Kind::Bool, Fixed, D::Bool(false)),
];

/// Parameters `ParametrizedFieldMapper.Builder.parse` drops with a
/// deprecation warning on a type that does not declare them.
pub(crate) const DEPRECATED_PARAMS: &[&str] = &[
    "store",
    "meta",
    "boost",
    "index",
    "doc_values",
    "index_options",
    "similarity",
];

/// `GeoPointFieldMapper`'s parameters, in `doXContentBody`'s order:
/// `FieldMapper`'s `index`, `doc_values` and `store`, `meta`, then the
/// geometry mapper's `ignore_malformed` and `ignore_z_value` (written only
/// when the mapping set them). `null_value` is refused as unsupported.
pub(crate) const GEO_POINT: &[Def] = &[
    p("index", Kind::Bool, Fixed, D::Bool(true)),
    p("doc_values", Kind::Bool, Fixed, D::Bool(true)),
    p("store", Kind::Bool, Fixed, D::Bool(false)),
    p("meta", Kind::Meta, Upd, D::Empty),
    p("ignore_malformed", Kind::Explicit, Upd, D::Bool(false)),
    p("ignore_z_value", Kind::Explicit, Upd, D::Bool(true)),
    only("null_value", Kind::NullString, Fixed, D::Null),
];

/// `KNNVectorFieldMapper.Builder.getParameters`, in its order. The stored,
/// doc-values, model, mode, compression and engine parameters are read and
/// refused as unsupported when set: the Lucene HNSW field this writer
/// indexes has none of them.
pub(crate) const KNN: &[Def] = &[
    only("store", Kind::Bool, Fixed, D::Bool(false)),
    only("doc_values", Kind::Bool, Fixed, D::Bool(false)),
    p(
        "dimension",
        Kind::KnnDimension,
        Fixed,
        D::Int(KNN_NO_DIMENSION),
    ),
    p("data_type", Kind::KnnDataType, Fixed, D::Str("float")),
    p("meta", Kind::Meta, Upd, D::Empty),
    p("method", Kind::KnnMethod, Fixed, D::Null),
    only("model_id", Kind::NullString, Fixed, D::Null),
    only("mode", Kind::NullString, Fixed, D::Null),
    only("compression_level", Kind::NullString, Fixed, D::Null),
    p("space_type", Kind::KnnSpace, Fixed, D::Str("undefined")),
    only("engine", Kind::NullString, Fixed, D::Str("undefined")),
];

/// `KNNVectorFieldMapper.UNSET_MODEL_DIMENSION_IDENTIFIER`: a dimension the mapping never set.
pub const KNN_NO_DIMENSION: i32 = -1;
