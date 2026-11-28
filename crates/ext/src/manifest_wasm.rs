//! The manifest of a WASM module: its descriptor (ADR 0052) plus an optional
//! `plugin` section, read into the one manifest a native plugin states
//! ([ADR 0080](../../../docs/adr/0080-one-plugin-system-three-tiers.md),
//! section 2), so one parser judges both.

use crate::descriptor::{Descriptor, DescriptorError};
use crate::json::Value;
use crate::manifest::{Manifest, ManifestError};

/// The keys a `plugin` section may hold.
const SECTION: &[&str] = &["api", "routes", "events", "capabilities", "budget"];

impl Manifest {
    /// Reads a module's descriptor as a plugin manifest: the id, version and
    /// points of the descriptor, the limits it lowers, and the `plugin`
    /// section's api, routes, events, capabilities and budget.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Invalid`] for a module with no `plugin` section (it is
    /// an extension module only, and has no plugin to mount) or for a key the
    /// manifest refuses, and [`ManifestError::Api`] for a plugin API this host
    /// does not speak.
    pub fn from_descriptor(d: &Descriptor) -> Result<Self, ManifestError> {
        let section = d
            .plugin_section()
            .ok_or_else(|| DescriptorError::at("plugin", "missing"))?;
        if let Some((k, _)) = section.iter().find(|(k, _)| !SECTION.contains(&k.as_str())) {
            return Err(DescriptorError::at(&format!("plugin.{k}"), "unknown key").into());
        }
        let api = section
            .iter()
            .find(|(k, _)| k == "api")
            .map(|(_, v)| v.clone())
            .ok_or_else(|| DescriptorError::at("plugin.api", "missing"))?;
        let (major, minor, patch) = d.version();
        // ALLOC: one manifest, at most the descriptor's points and sections.
        let mut items = vec![
            (
                "plugin".to_owned(),
                Value::Obj(vec![
                    ("id".to_owned(), Value::Str(d.name().to_owned())),
                    (
                        "version".to_owned(),
                        Value::Str(format!("{major}.{minor}.{patch}")),
                    ),
                    ("api".to_owned(), api),
                ]),
            ),
            (
                "points".to_owned(),
                Value::Arr(
                    d.points()
                        .iter()
                        .map(|p| {
                            Value::Obj(vec![
                                ("name".to_owned(), Value::Str(p.id().to_owned())),
                                ("version".to_owned(), Value::Int(u64::from(p.version().0))),
                            ])
                        })
                        .collect(),
                ),
            ),
        ];
        items.extend(section.iter().filter(|(k, _)| k != "api").cloned());
        let l = d.limits();
        let lowered: Vec<(String, Value)> = [
            ("fuel", l.fuel),
            ("memory_bytes", l.memory_bytes),
            ("deadline_ms", l.deadline_ms),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k.to_owned(), Value::Int(v))))
        .collect();
        if !lowered.is_empty() {
            items.push(("limits".to_owned(), Value::Obj(lowered)));
        }
        Manifest::from_items(items)
    }
}
