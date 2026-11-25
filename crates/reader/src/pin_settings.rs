//! The analyzer pins an index's settings hold
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)), read
//! from the flat settings a descriptor carries.

use oss_ext::pin::{Pin, ResolvedLimits, read_pins};
use oss_format::{ClusterSettings, Setting};

use crate::error::ReaderError;
use crate::mapping::MappingError;

/// The pins `settings` hold, each with the limits recorded for it, sorted by
/// name; none for an index that pins nothing.
///
/// # Errors
///
/// [`ReaderError::Mapping`] when the pin settings do not read: the node
/// that wrote them wrote a pin this node cannot understand, so the index is
/// not opened.
pub fn pins_of(settings: &ClusterSettings) -> Result<Vec<(Pin, ResolvedLimits)>, ReaderError> {
    // ALLOC: the pin keys of one index, at most the pins' settings.
    let flat: Vec<(String, Option<String>)> = settings
        .settings()
        .iter()
        .filter(|(k, _)| k.starts_with("index.analysis."))
        .map(|(k, v)| {
            let text = match v {
                Setting::Value(v) => Some(v.clone()),
                Setting::List(_) => None,
            };
            (k.clone(), text)
        })
        // ALLOC: the pin keys of one index.
        .collect();
    read_pins(&flat).map_err(|e| {
        // ALLOC: error path; one message.
        ReaderError::Mapping(MappingError(format!(
            "analyzer pin [{}]: {}",
            e.key(),
            e.reason()
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn settings(json: &str) -> ClusterSettings {
        ClusterSettings::parse(json.as_bytes()).expect("settings")
    }

    #[test]
    fn an_index_with_no_pin_has_none_and_one_that_pins_has_its_pins_and_limits() {
        assert!(
            pins_of(&settings(r#"{"index.number_of_shards":"1"}"#))
                .expect("reads")
                .is_empty()
        );
        let json = format!(
            r#"{{"index.analysis.analyzer.ext_a.type":"serverless_module",
            "index.analysis.analyzer.ext_a.module":"{HASH}",
            "index.analysis.analyzer.ext_a.version":"1",
            "index.analysis.analyzer.ext_a.limits.fuel":"500",
            "index.analysis.analyzer.ext_a.limits.memory_bytes":"1048576",
            "index.analysis.analyzer.ext_a.limits.deadline_ms":"50"}}"#
        );
        let pins = pins_of(&settings(&json)).expect("reads");
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].0.name, "ext_a");
        assert_eq!(pins[0].0.module, HASH);
        assert_eq!(pins[0].1.fuel, 500);
    }

    #[test]
    fn a_pin_that_does_not_read_refuses_the_open_in_words() {
        let e = pins_of(&settings(
            r#"{"index.analysis.analyzer.ext_a.type":"serverless_module"}"#,
        ))
        .expect_err("refused");
        assert!(e.to_string().contains("ext_a"), "{e}");
    }
}
