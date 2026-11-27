//! Reads one parameter's value from its JSON, as its parameter kind says.

use super::error::MappingError;
use super::json::Json;
use super::numbers;
use super::params::{Def, Kind, Value, parse_bool, parse_float, parse_int, parse_meta};
use super::types::FieldType;

/// Reads parameter `def` of a field `name` of type `ty` from `node`.
///
/// # Errors
///
/// OpenSearch's messages for a value of the wrong shape, or a `null` where the parameter takes none.
pub(super) fn parse_value(
    name: &str,
    ty: FieldType,
    def: &Def,
    node: &Json,
) -> Result<Value, MappingError> {
    let nullable = matches!(
        def.kind,
        Kind::NullString | Kind::NullBool | Kind::NullNumber
    );
    if *node == Json::Null {
        if nullable {
            return Ok(Value::Null);
        }
        // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
        return Err(MappingError::mapper_parsing(&format!(
            "[{}] on mapper [{name}] of type [{}] must not have a [null] value",
            def.name,
            ty.name()
        )));
    }
    Ok(match def.kind {
        Kind::Bool => Value::Bool(parse_bool(node)?),
        Kind::Explicit => Value::Explicit(parse_bool(node)?, true),
        Kind::Int => Value::Int(parse_int(node)?),
        Kind::Boost => Value::Float(parse_float(node)?),
        Kind::Str | Kind::Analyzer | Kind::NullString => Value::Str(node.java_string()),
        Kind::Choice(allowed) => {
            let v = node.java_string();
            if !allowed.contains(&v.as_str()) {
                // ALLOC: mapping parse, merge or error path; bounded by the mapping text (MAX_MAPPING_BYTES).
                return Err(MappingError::mapper_parsing(&format!(
                    "Unknown value [{v}] for field [{}] - accepted values are [{}]",
                    def.name,
                    allowed.join(", ")
                )));
            }
            Value::Str(v)
        }
        Kind::Meta => Value::Meta(parse_meta(name, node)?),
        Kind::NullBool => Value::Bool(parse_bool(node)?),
        Kind::KnnDimension => Value::Int(super::knn::parse_dimension(name, node)?),
        Kind::KnnDataType => Value::Str(super::knn::parse_data_type(node)?),
        Kind::KnnMethod => Value::Method(Box::new(super::knn::parse_method(node)?)),
        Kind::KnnSpace => Value::Str(super::knn::parse_space(node)?),
        Kind::ScalingFactor => Value::Double(numbers::scaling_factor(node)?),
        Kind::NullNumber => match ty {
            FieldType::Number(n) => Value::Number(numbers::parse(n, node, false)?),
            _ => Value::Null,
        },
    })
}
