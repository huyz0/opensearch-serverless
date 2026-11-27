//! The `double` field `d`, the `float` field `f` and the `half_float` field `h` of a
//! [`Spec::with_decimals`] index.

use lucene_codecs::field_infos::{DocValuesSkipIndexType, DocValuesType, FieldInfo};
use lucene_codecs::stored_fields::{FieldValue, StoredField};
use lucene_index::index_writer::{ExplicitDocument, IndexWriter};
use lucene_util::half_float::half_float_to_sortable_short;

use super::Spec;

/// The fields' numbers: registered right after `n`, so the keywords follow them.
const DOUBLE: i32 = 4;
const FLOAT: i32 = 5;
const HALF: i32 = 6;

/// How many fields this module registers.
pub(super) const FIELDS: i32 = 3;

/// `NumericUtils.doubleToSortableLong`.
fn double_to_sortable_long(v: f64) -> i64 {
    let bits = v.to_bits().cast_signed();
    bits ^ ((bits >> 63) & i64::MAX)
}

/// `NumericUtils.floatToSortableInt`.
fn float_to_sortable_int(v: f32) -> i32 {
    let bits = v.to_bits().cast_signed();
    bits ^ ((bits >> 31) & i32::MAX)
}

/// Document `i`'s `double` field `d` when [`Spec::with_decimals`] is set: a quarter steps from -5.
pub fn doc_double(i: usize) -> f64 {
    // ARITH: test sizes are small; the conversion is exact below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let n = i as f64;
    n * 0.25 - 5.0
}

/// Document `i`'s `float` field `f` when [`Spec::with_decimals`] is set: a tenth, which a float
/// holds inexactly, so a test sees the widening to a double.
pub fn doc_float(i: usize) -> f32 {
    // ARITH: test sizes are small; the conversion is exact below 2^24.
    #[allow(clippy::cast_precision_loss)]
    let n = i as f32;
    n * 0.1
}

/// Document `i`'s `half_float` field `h` when [`Spec::with_decimals`] is set: a tenth, which a
/// half holds more coarsely than a float (`0.1` is stored as `0.0999755859375`).
pub fn doc_half(i: usize) -> f32 {
    doc_float(i)
}

/// The fields' mapping, when `spec` has them.
pub(super) fn mapping(spec: &Spec, out: &mut String) {
    if spec.decimals {
        out.push_str(r#","d":{"type":"double"},"f":{"type":"float"},"h":{"type":"half_float"}"#);
    }
}

/// Document `i`'s values in its `_source`.
pub(super) fn source(spec: &Spec, i: usize, out: &mut String) {
    if spec.decimals {
        out.push_str(&format!(
            r#","d":{:?},"f":{:?},"h":{:?}"#,
            doc_double(i),
            doc_float(i),
            doc_half(i)
        ));
    }
}

/// Registers the fields with the writer.
pub(super) fn register(w: &mut IndexWriter<'_>, spec: &Spec) {
    if !spec.decimals {
        return;
    }
    let numeric = DocValuesType::SortedNumeric;
    w.register_field(
        FieldInfo::new("d", DOUBLE)
            .with_points(1, 1, 8)
            .with_doc_values(numeric, DocValuesSkipIndexType::None, -1),
    )
    .expect("d");
    w.register_field(
        FieldInfo::new("f", FLOAT)
            .with_points(1, 1, 4)
            .with_doc_values(numeric, DocValuesSkipIndexType::None, -1),
    )
    .expect("f");
    w.register_field(
        FieldInfo::new("h", HALF)
            .with_points(1, 1, 2)
            .with_doc_values(numeric, DocValuesSkipIndexType::None, -1),
    )
    .expect("h");
}

/// Adds document `i`'s doc values and points.
pub(super) fn fields(spec: &Spec, i: usize, doc: &mut ExplicitDocument) {
    if !spec.decimals {
        return;
    }
    let d = double_to_sortable_long(doc_double(i));
    let f = float_to_sortable_int(doc_float(i));
    let h = half_float_to_sortable_short(doc_half(i));
    let dv = &mut doc.fields.doc_values;
    dv.push(StoredField {
        field_number: DOUBLE,
        value: FieldValue::Long(d),
    });
    dv.push(StoredField {
        field_number: FLOAT,
        value: FieldValue::Long(i64::from(f)),
    });
    dv.push(StoredField {
        field_number: HALF,
        value: FieldValue::Long(i64::from(h)),
    });
    let points = &mut doc.fields.points;
    points.push(StoredField {
        field_number: DOUBLE,
        value: FieldValue::Binary((d ^ i64::MIN).to_be_bytes().to_vec()),
    });
    points.push(StoredField {
        field_number: FLOAT,
        value: FieldValue::Binary((f ^ i32::MIN).to_be_bytes().to_vec()),
    });
    points.push(StoredField {
        field_number: HALF,
        value: FieldValue::Binary((h ^ i16::MIN).to_be_bytes().to_vec()),
    });
}
