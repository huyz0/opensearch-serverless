//! A mapping's plan for the ingest: the fields of the mapping that a document reads exactly as
//! the complete path reads them, in the blob layout `lucene_engine::ingest::register` takes.

use std::collections::HashMap;
use std::sync::Arc;

use lucene_codecs::field_infos::IndexOptions;
use lucene_engine::ingest::{self, Plan};
use lucene_engine::{analysis, status::Status};

use crate::lucene::{Indexer, MetaField};
use crate::lucene::{Shape, Spec};
use crate::mapping::{FieldType, Leaf, Mapping, NumType};

/// `index.mapping.nested_objects.limit`'s default; a plan has no nested field.
const NESTED_LIMIT: i32 = 10_000;
/// The analysis spec layout's version byte.
const SPEC_VERSION: u8 = 1;
/// `StandardTokenizer`'s default token length.
const MAX_TOKEN_LENGTH: &str = "255";
/// The similarity's `toString()`, which only an error names.
const SIMILARITY: &str = "BM25(k1=1.2,b=0.75)";

/// The parameters a field may set and still be read by the plan; every other parameter must
/// hold its default.
const FREE: &[&str] = &[
    "index",
    "doc_values",
    "norms",
    "ignore_above",
    "index_options",
    "position_increment_gap",
    "eager_global_ordinals",
    "boost",
    "meta",
];

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_i32(out, i32::try_from(b.len()).unwrap_or(i32::MAX));
    out.extend_from_slice(b);
}

/// Whether `leaf` sets nothing the plan does not read: a type of the plan's kinds, and every
/// parameter outside [`FREE`] at its default.
fn plain(leaf: &Leaf) -> bool {
    if matches!(leaf.ty(), FieldType::Number(NumType::Byte | NumType::Short)) {
        return false;
    }
    let default = Leaf::new(leaf.ty());
    leaf.ty()
        .defs()
        .iter()
        .filter(|d| !FREE.contains(&d.name))
        .all(|d| leaf.get(d.name) == default.get(d.name))
}

/// The analysis registration of the standard analyzer with position increment gap `gap`.
fn analyzer(gap: i32) -> Result<u32, Status> {
    let mut spec = vec![SPEC_VERSION];
    put_i32(&mut spec, gap);
    // The offset gap of `Analyzer.getOffsetGap`'s default.
    put_i32(&mut spec, 1);
    put_i32(&mut spec, 0);
    put_bytes(&mut spec, b"standard");
    put_i32(&mut spec, 1);
    put_bytes(&mut spec, b"maxTokenLength");
    put_bytes(&mut spec, MAX_TOKEN_LENGTH.as_bytes());
    put_i32(&mut spec, 1);
    put_bytes(&mut spec, b"lowercase");
    put_i32(&mut spec, 0);
    put_i32(&mut spec, 0);
    let mut blob = Vec::new();
    put_bytes(&mut blob, &spec);
    put_bytes(&mut blob, SIMILARITY.as_bytes());
    analysis::register(&blob)
}

fn index_options_ordinal(o: IndexOptions) -> Option<u8> {
    Some(match o {
        IndexOptions::None => 0,
        IndexOptions::Docs => 1,
        IndexOptions::DocsAndFreqs => 2,
        IndexOptions::DocsAndFreqsAndPositions => 3,
        IndexOptions::DocsAndFreqsAndPositionsAndOffsets => 4,
        _ => return None,
    })
}

/// The plan blob's "no field number" and "no parent": the engine reads a negative as absent.
const NONE: i32 = -1;

/// The entry's flag bits: bit 0 indexed (terms or points), bit 1 doc values.
fn flags(indexed: bool, doc_values: bool) -> u8 {
    let mut bits = 0_u8;
    if indexed {
        bits = bits.saturating_add(1);
    }
    if doc_values {
        bits = bits.saturating_add(2);
    }
    bits
}

struct Entry {
    name: String,
    kind: u8,
    flags: u8,
    number: i32,
    ignore_above: i32,
    analyzer: i32,
    options: u8,
    norm: u8,
    parent: i32,
}

/// The entry `leaf` is in a plan, or `None` when the plan cannot read it.
fn entry(name: &str, leaf: &Leaf, spec: Spec, number: i32) -> Option<Entry> {
    if !plain(leaf) {
        return None;
    }
    // A stored field, one that adds nothing and one the engine would have to name in
    // `_field_names` stay with the complete path.
    if spec.store {
        return None;
    }
    if !spec.adds_anything() {
        return None;
    }
    if spec.field_names() {
        return None;
    }
    let mut e = Entry {
        name: name.to_owned(),
        kind: 0,
        flags: flags(spec.inverted() || spec.points, spec.doc_values),
        number,
        ignore_above: i32::MAX,
        analyzer: 0,
        options: 0,
        norm: 0,
        parent: NONE,
    };
    match spec.shape {
        Shape::Keyword => {
            // The ingest writes a term once with no frequency and no norm.
            if spec.norms || spec.index_options == IndexOptions::DocsAndFreqs {
                return None;
            }
            e.kind = ingest::KEYWORD;
            if let Some(crate::mapping::Value::Int(i)) = leaf.get("ignore_above") {
                e.ignore_above = *i;
            }
        }
        Shape::Text { gap } => {
            e.kind = ingest::TEXT;
            e.analyzer = i32::try_from(analyzer(gap).ok()?).ok()?;
            e.options = index_options_ordinal(spec.index_options)?;
            // 0 omits the norm, 1 is `Similarity.computeNorm` discounting overlaps.
            e.norm = u8::from(spec.norms);
        }
        Shape::Int => e.kind = ingest::INTEGER,
        Shape::Long => e.kind = ingest::LONG,
        Shape::Float => e.kind = ingest::FLOAT,
        Shape::Double => e.kind = ingest::DOUBLE,
        // The ingest has no half-float kind, and a scaled value is the JSON number times the factor.
        Shape::HalfFloat | Shape::Scaled => return None,
        Shape::Date => e.kind = ingest::DATE,
        Shape::Bool => e.kind = ingest::BOOLEAN,
        // A vector goes through the complete path, which hands the engine its vectors.
        Shape::Vector | Shape::GeoPoint => return None,
    }
    Some(e)
}

/// The plan of `mapping`'s fields that `indexer` has numbered with the spec the mapping gives
/// them, or `None` when no metadata field is numbered yet or the ingest refuses the plan.
pub(super) fn compile(indexer: &Indexer, mapping: &Mapping) -> Option<Arc<Plan>> {
    let meta = [
        MetaField::Id,
        MetaField::SeqNo,
        MetaField::PrimaryTerm,
        MetaField::Version,
        MetaField::Source,
    ]
    .map(|m| indexer.meta_number(m));
    let [
        Some(id),
        Some(seq_no),
        Some(primary_term),
        Some(version),
        Some(source),
    ] = meta
    else {
        return None;
    };
    let numbered: HashMap<&str, (i32, Option<Spec>)> = indexer
        .numbered()
        .map(|(n, number, spec)| (n, (number, spec)))
        .collect();
    let leaves = mapping.leaves();
    // Each multi-field's parent, by full name.
    let mut parents: HashMap<String, &str> = HashMap::new();
    for (name, leaf) in &leaves {
        for (sub, _) in leaf.multi_fields() {
            parents.insert(crate::mapping::join(name, sub), name);
        }
    }
    // The entry each field has in a plan, when the plan can read it.
    let mut ready: HashMap<&str, Entry> = HashMap::new();
    for (name, leaf) in &leaves {
        let Some(&(number, Some(registered))) = numbered.get(name.as_str()) else {
            continue;
        };
        let spec = Spec::of(leaf);
        if spec != registered {
            continue;
        }
        if let Some(e) = entry(name, leaf, spec, number) {
            ready.insert(name.as_str(), e);
        }
    }
    // A field with multi-fields is read only with all of them: a document's value would add to
    // the ones the plan lacks, and the plan must not drop that silently.
    let mut kept: Vec<Entry> = Vec::new();
    for (name, leaf) in &leaves {
        if parents.contains_key(name.as_str()) {
            continue;
        }
        let subs: Vec<String> = leaf
            .multi_fields()
            .iter()
            .map(|(sub, _)| crate::mapping::join(name, sub))
            .collect();
        if !ready.contains_key(name.as_str())
            || !subs.iter().all(|s| ready.contains_key(s.as_str()))
        {
            continue;
        }
        let Some(parent) = ready.remove(name.as_str()) else {
            continue;
        };
        let at = i32::try_from(kept.len()).ok()?;
        kept.push(parent);
        for sub in &subs {
            let Some(mut e) = ready.remove(sub.as_str()) else {
                continue;
            };
            e.parent = at;
            kept.push(e);
        }
    }
    if kept.is_empty() {
        return None;
    }
    // The objects a dotted name passes through.
    let mut objects: Vec<String> = Vec::new();
    for e in &kept {
        let mut end = 0;
        while let Some(dot) = e.name.get(end..).and_then(|r| r.find('.')) {
            end = end.saturating_add(dot);
            let path = &e.name[..end];
            if !kept.iter().any(|k| k.name == path) && !objects.iter().any(|o| o == path) {
                objects.push(path.to_owned());
            }
            end = end.saturating_add(1);
        }
    }
    // Object entries come after the leaves, so a multi-field's parent index is unchanged.
    let total = kept.len().saturating_add(objects.len());
    let mut blob = Vec::new();
    put_i32(&mut blob, i32::try_from(total).ok()?);
    for e in &kept {
        put_bytes(&mut blob, e.name.as_bytes());
        blob.push(e.kind);
        blob.push(e.flags);
        put_i32(&mut blob, e.number);
        put_i32(&mut blob, e.ignore_above);
        put_i32(&mut blob, e.analyzer);
        blob.push(e.options);
        blob.push(e.norm);
        put_i32(&mut blob, e.parent);
        put_i32(&mut blob, 0);
    }
    for o in &objects {
        put_bytes(&mut blob, o.as_bytes());
        blob.push(ingest::OBJECT);
        blob.push(0);
        put_i32(&mut blob, NONE);
        put_i32(&mut blob, i32::MAX);
        put_i32(&mut blob, 0);
        blob.push(0);
        blob.push(0);
        put_i32(&mut blob, NONE);
        put_i32(&mut blob, 0);
    }
    put_i32(&mut blob, NESTED_LIMIT);
    for n in [id, seq_no, primary_term, version, source, NONE] {
        put_i32(&mut blob, n);
    }
    // `_seq_no` is a point as well as a doc value, as `Indexer::document` writes it.
    blob.push(1);
    put_i32(&mut blob, 0);
    let plan_id = ingest::register(&blob).ok()?;
    ingest::plan(plan_id)
}
