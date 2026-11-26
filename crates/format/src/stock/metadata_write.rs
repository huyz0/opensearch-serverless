//! The JSON of the three metadata blobs an export writes besides
//! `index-N` and the shard blobs: a snapshot's own record, the (empty)
//! global metadata and an index's metadata, laid out as a stock node writes
//! them. Each result goes through [`super::from_json`] into the envelope.

use crate::error::{FormatError, corrupt};
use crate::index_record::MAX_SHARDS;
use crate::json::Reader;
use crate::json_write::{push_string, push_u64};

use super::repository_data::is_blob_name;

const WHAT: &str = "stock metadata";

/// The version id a snapshot records: 3.10.0 with OpenSearch's mask bit.
pub const VERSION_ID: u64 = 137_317_827;
/// The setting `index.version.created` an exported index carries.
const VERSION_CREATED: &str = "137317827";
/// The routing shard count a stock index is created with at most.
const LOG2_MAX_ROUTING_SHARDS: u32 = 10;
/// The settings the export owns, so a caller's copy of one is dropped.
const OWNED: [&str; 7] = [
    "index.creation_date",
    "index.number_of_replicas",
    "index.number_of_shards",
    "index.provided_name",
    "index.replication.type",
    "index.uuid",
    "index.version.created",
];

/// A snapshot's own record, as the take writes it.
#[derive(Debug, Clone, Copy)]
pub struct NewSnapshotInfo<'a> {
    /// The snapshot's name.
    pub name: &'a str,
    /// Its uuid.
    pub uuid: &'a str,
    /// The names of the indices it holds.
    pub indices: &'a [&'a str],
    /// When the take began, in epoch milliseconds.
    pub start_time_ms: u64,
    /// When it ended.
    pub end_time_ms: u64,
    /// How many shards it covers.
    pub total_shards: u64,
    /// How many of them it holds.
    pub successful_shards: u64,
}

/// The JSON of a successful snapshot's own blob, appended to `out`.
pub fn snapshot_info_json(new: &NewSnapshotInfo<'_>, out: &mut Vec<u8>) -> Result<(), FormatError> {
    if new.name.is_empty() || !is_blob_name(new.uuid) {
        return Err(corrupt(WHAT, "a snapshot without a name or a safe uuid"));
    }
    out.extend_from_slice(b"{\"snapshot\":{\"name\":");
    push_string(out, new.name);
    out.extend_from_slice(b",\"uuid\":");
    push_string(out, new.uuid);
    out.extend_from_slice(b",\"version_id\":");
    push_u64(out, VERSION_ID);
    out.extend_from_slice(b",\"remote_store_index_shallow_copy\":false,\"indices\":[");
    for (n, index) in new.indices.iter().enumerate() {
        if n > 0 {
            out.push(b',');
        }
        push_string(out, index);
    }
    out.extend_from_slice(
        b"],\"data_streams\":[],\"state\":\"SUCCESS\",\"include_global_state\":false,\"metadata\":null,\"start_time\":",
    );
    push_u64(out, new.start_time_ms);
    out.extend_from_slice(b",\"end_time\":");
    push_u64(out, new.end_time_ms);
    out.extend_from_slice(b",\"total_shards\":");
    push_u64(out, new.total_shards);
    out.extend_from_slice(b",\"successful_shards\":");
    push_u64(out, new.successful_shards);
    out.extend_from_slice(b",\"failures\":[]}}");
    Ok(())
}

/// The JSON of the global metadata blob a snapshot without global state
/// holds: no templates, no coordination state.
pub fn global_metadata_json(out: &mut Vec<u8>) -> Result<(), FormatError> {
    out.extend_from_slice(
        b"{\"meta-data\":{\"version\":0,\"cluster_uuid\":\"_na_\",\"cluster_uuid_committed\":false,\"cluster_coordination\":{\"term\":0,\"last_committed_config\":[],\"last_accepted_config\":[],\"voting_config_exclusions\":[]},\"templates\":{},\"data_stream\":{\"data_stream\":{}}}}",
    );
    Ok(())
}

/// An index to describe to a stock node.
#[derive(Debug, Clone, Copy)]
pub struct NewIndexMetadata<'a> {
    /// The index's name.
    pub name: &'a str,
    /// The uuid in `index.uuid`.
    pub uuid: &'a str,
    /// How many shards, 1 to the descriptor's most.
    pub number_of_shards: u32,
    /// `index.creation_date`, in epoch milliseconds.
    pub creation_date_ms: u64,
    /// The `_doc` mapping as a JSON object, when the index has one.
    pub mapping: Option<&'a str>,
    /// The settings to carry beside the ones this writer owns.
    pub settings: &'a [(String, String)],
    /// One allocation id a shard.
    pub allocation_ids: &'a [String],
}

/// What a stock create computes for the shard count routing is spread over:
/// `shards * 2^(10 - ceil(log2 shards))`, at least `shards`.
fn routing_shards(shards: u32) -> u64 {
    let log2 = 32u32.saturating_sub(shards.saturating_sub(1).leading_zeros());
    let splits = LOG2_MAX_ROUTING_SHARDS.saturating_sub(log2);
    u64::from(shards).saturating_mul(1u64.wrapping_shl(splits))
}

/// A JSON object, exactly one: nothing may follow it.
fn check_object(text: &str) -> Result<(), FormatError> {
    let mut r = Reader::new(text.as_bytes(), WHAT);
    r.start_object("a mapping is an object")?;
    while r.next_field()?.is_some() {
        r.skip_value()?;
    }
    r.finish()
}

/// The JSON of an index's metadata blob, appended to `out`; a refusal leaves
/// `out` as it was.
pub fn index_metadata_json(
    new: &NewIndexMetadata<'_>,
    out: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let shards = usize::try_from(new.number_of_shards).unwrap_or(usize::MAX);
    if new.name.is_empty() || !is_blob_name(new.uuid) {
        return Err(corrupt(WHAT, "an index without a name or a safe uuid"));
    }
    if new.number_of_shards == 0 || new.number_of_shards > MAX_SHARDS {
        return Err(corrupt(WHAT, "an index whose shard count is out of range"));
    }
    if new.allocation_ids.len() != shards || !new.allocation_ids.iter().all(|a| is_blob_name(a)) {
        return Err(corrupt(WHAT, "an allocation id a shard, each safe"));
    }
    if let Some(mapping) = new.mapping {
        check_object(mapping)?;
    }
    write_index(new, shards, out);
    Ok(())
}

fn write_index(new: &NewIndexMetadata<'_>, shards: usize, out: &mut Vec<u8>) {
    out.push(b'{');
    push_string(out, new.name);
    out.extend_from_slice(b":{\"version\":3,\"mapping_version\":1,\"settings_version\":1,\"aliases_version\":1,\"routing_num_shards\":");
    push_u64(out, routing_shards(new.number_of_shards));
    out.extend_from_slice(b",\"state\":\"open\",\"settings\":{");
    // ALLOC: one pair a setting the caller carries, and the owned seven.
    let mut all: Vec<(&str, String)> = vec![
        ("index.creation_date", new.creation_date_ms.to_string()), // ALLOC: one value.
        ("index.number_of_replicas", "0".to_owned()),              // ALLOC: one value.
        ("index.number_of_shards", new.number_of_shards.to_string()), // ALLOC: one value.
        ("index.provided_name", new.name.to_owned()),              // ALLOC: one value.
        ("index.replication.type", "DOCUMENT".to_owned()),         // ALLOC: one value.
        ("index.uuid", new.uuid.to_owned()),                       // ALLOC: one value.
        ("index.version.created", VERSION_CREATED.to_owned()),     // ALLOC: one value.
    ];
    for (key, value) in new.settings {
        if !OWNED.contains(&key.as_str()) {
            all.push((key.as_str(), value.clone())); // ALLOC: one value a setting.
        }
    }
    all.sort_by(|a, b| a.0.cmp(b.0));
    for (n, (key, value)) in all.iter().enumerate() {
        if n > 0 {
            out.push(b',');
        }
        push_string(out, key);
        out.push(b':');
        push_string(out, value);
    }
    out.extend_from_slice(b"},\"mappings\":[");
    if let Some(mapping) = new.mapping {
        out.extend_from_slice(b"{\"_doc\":");
        out.extend_from_slice(mapping.as_bytes());
        out.push(b'}');
    }
    out.extend_from_slice(b"],\"aliases\":{},\"primary_terms\":[");
    for n in 0..shards {
        if n > 0 {
            out.push(b',');
        }
        out.push(b'1');
    }
    out.extend_from_slice(b"],\"primary_terms_map\":{");
    for n in 0..shards {
        if n > 0 {
            out.push(b',');
        }
        out.extend_from_slice(format!("\"{n}\":1").as_bytes()); // ALLOC: one number a shard.
    }
    out.extend_from_slice(b"},\"in_sync_allocations\":{");
    for (n, id) in new.allocation_ids.iter().enumerate() {
        if n > 0 {
            out.push(b',');
        }
        out.extend_from_slice(format!("\"{n}\":[").as_bytes()); // ALLOC: one number a shard.
        push_string(out, id);
        out.push(b']');
    }
    out.extend_from_slice(
        b"},\"rollover_info\":{},\"system\":false,\"ingestion_status\":{\"is_paused\":false},\"split_shards_metadata\":{\"num_of_root_shards\":",
    );
    push_u64(out, u64::from(new.number_of_shards));
    out.extend_from_slice(b",\"max_shard_id\":");
    push_u64(out, u64::from(new.number_of_shards.saturating_sub(1)));
    out.extend_from_slice(b",\"active_shard_ids\":[");
    for n in 0..shards {
        if n > 0 {
            out.push(b',');
        }
        out.extend_from_slice(n.to_string().as_bytes()); // ALLOC: one number a shard.
    }
    out.extend_from_slice(b"],\"root_shards_to_all_children\":{},\"parent_to_child_shards\":{}}}}");
}
