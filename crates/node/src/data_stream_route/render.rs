//! The body of a data stream read, as core renders it.

use oss_format::AliasRecord;

/// `s` as a JSON string.
fn quoted(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len().saturating_add(2));
    // Writing to a Vec cannot fail.
    let _ = oss_rest::write_json_str(&mut out, s);
    String::from_utf8_lossy(&out).into_owned()
}

/// `{"data_streams":[..]}` for one stream: each backing index with the uuid
/// its descriptor holds (`None` for one with no descriptor, which is RED).
pub(super) fn stream(
    record: &AliasRecord<'_>,
    backing: &[(String, Option<String>)],
    template: Option<&str>,
) -> String {
    let timestamp = record.timestamp_field.as_deref().unwrap_or("@timestamp");
    let red = backing.iter().any(|(_, uuid)| uuid.is_none());
    let mut out = String::from("{\"data_streams\":[{\"name\":");
    out.push_str(&quoted(&record.name));
    out.push_str(",\"timestamp_field\":{\"name\":");
    out.push_str(&quoted(timestamp));
    out.push_str("},\"indices\":[");
    for (n, (index, uuid)) in backing.iter().enumerate() {
        if n > 0 {
            out.push(',');
        }
        out.push_str("{\"index_name\":");
        out.push_str(&quoted(index));
        out.push_str(",\"index_uuid\":");
        out.push_str(&quoted(uuid.as_deref().unwrap_or("_na_")));
        out.push('}');
    }
    out.push_str("],\"generation\":");
    out.push_str(&record.generation.to_string());
    out.push_str(",\"status\":\"");
    out.push_str(if red { "RED" } else { "GREEN" });
    out.push('"');
    if let Some(template) = template {
        out.push_str(",\"template\":");
        out.push_str(&quoted(template));
    }
    out.push_str("}]}");
    out
}
