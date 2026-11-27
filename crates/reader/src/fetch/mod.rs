//! The fetch phase's per-hit features: what each hit carries beyond its
//! `_id` and score.

pub mod docvalue_fields;
pub mod explain;
pub mod highlight;
pub mod highlight_parse;
pub mod hit_meta;
pub mod nested_source;
pub mod script_fields;
pub mod source_fields;
pub mod source_filter;
mod source_map;
pub mod stored_fields;
