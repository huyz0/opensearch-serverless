//! `OSS_ROLES`: the roles a node's lease names.

impl super::NodeConfig {
    /// Whether the node takes writes: its lease names `ingest`.
    pub fn takes_writes(&self) -> bool {
        self.roles.contains(&"ingest")
    }
}

/// The roles a node can take.
const KNOWN: [&str; 2] = ["ingest", "search"];

/// Reads the role list: unset or empty is both roles, `none` alone is no
/// role, anything else is a comma list of known roles, in the order of
/// [`KNOWN`] and with repeats counted once.
pub(in crate::boot) fn read(value: Option<String>) -> Result<Vec<&'static str>, String> {
    let value = value.unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        return Ok(KNOWN.to_vec());
    }
    let named: Vec<&str> = value.split(',').map(str::trim).collect();
    if named.contains(&"none") {
        return if named.len() == 1 {
            Ok(Vec::new())
        } else {
            Err("OSS_ROLES [none] cannot be listed beside another role".to_owned())
        };
    }
    if let Some(bad) = named.iter().find(|n| !KNOWN.contains(n)) {
        return Err(format!(
            "OSS_ROLES names [{bad}], not ingest, search or none"
        ));
    }
    Ok(KNOWN
        .iter()
        .copied()
        .filter(|k| named.contains(k))
        .collect())
}
