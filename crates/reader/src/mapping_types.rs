//! What a [`FieldType`] is called when a refusal names it, and the `knn_vector` field's space.

use crate::mapping::FieldType;

impl FieldType {
    /// How a refusal names a type this reader does not serve in some place
    /// yet, as it names an [`FieldType::Unsupported`] one.
    pub fn refusal_name(&self) -> &str {
        match self {
            FieldType::Float => "a [float] field",
            FieldType::Double => "a [double] field",
            FieldType::HalfFloat => "a [half_float] field",
            FieldType::Scaled(_) => "a [scaled_float] field",
            FieldType::GeoPoint => "a [geo_point] field",
            FieldType::Unsupported(what) => what,
            FieldType::Text => "a [text] field",
            FieldType::Keyword => "a [keyword] field",
            FieldType::Int { name, .. } => name,
            FieldType::Long => "a [long] field",
            FieldType::Date => "a [date] field",
        }
    }
}

impl FieldType {
    /// The `scaling_factor` of a `scaled_float` field.
    pub fn scaling_factor(&self) -> Option<f64> {
        match self {
            FieldType::Scaled(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }
}

/// The space a `knn_vector` field scores in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// `l2`, the default.
    L2,
    /// `cosinesimil`.
    CosineSimil,
    /// `innerproduct`.
    InnerProduct,
}

impl Space {
    /// The name the plugin's messages give the space (`SpaceType.name()`).
    pub fn name(self) -> &'static str {
        match self {
            Space::L2 => "L2",
            Space::CosineSimil => "COSINESIMIL",
            Space::InnerProduct => "INNER_PRODUCT",
        }
    }
}

/// What the `knn` query reads of a `knn_vector` field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnnField {
    /// The vectors' length.
    pub dimension: u32,
    /// The space the field's `space_type` names.
    pub space: Space,
}
