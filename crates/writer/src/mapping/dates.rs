//! A `date` field's `format`: the `||`-separated formats this writer reads.
//!
//! `DateFormatter.forPattern` takes any of OpenSearch's built-in names and
//! any `java.time` pattern. This writer takes the default
//! (`strict_date_optional_time||epoch_millis`), the formats dynamic mapping
//! gives a date (`yyyy/MM/dd HH:mm:ss`, `yyyy/MM/dd`), and the epoch
//! formats; a field with any other format is refused as unsupported, so its
//! index stays with a Java writer.

/// One format this writer reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `strict_date_optional_time`
    StrictDateOptionalTime,
    /// `date_optional_time`
    DateOptionalTime,
    /// `epoch_millis`
    EpochMillis,
    /// `epoch_second`
    EpochSecond,
    /// `yyyy/MM/dd HH:mm:ss`
    SlashDateTime,
    /// `yyyy/MM/dd`
    SlashDate,
}

impl Format {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "strict_date_optional_time" | "strictDateOptionalTime" => {
                Format::StrictDateOptionalTime
            }
            "date_optional_time" | "dateOptionalTime" => Format::DateOptionalTime,
            "epoch_millis" => Format::EpochMillis,
            "epoch_second" => Format::EpochSecond,
            "yyyy/MM/dd HH:mm:ss" => Format::SlashDateTime,
            "yyyy/MM/dd" => Format::SlashDate,
            _ => return None,
        })
    }
}

/// A field's formats, tried in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formats(pub Vec<Format>);

impl Formats {
    /// The formats a `format` names, or `None` when this writer does not
    /// read one of them.
    pub fn parse(pattern: &str) -> Option<Self> {
        // ALLOC: one entry a format, bounded by the pattern's length.
        let mut out = Vec::new();
        for name in pattern.split("||") {
            out.push(Format::from_name(name)?);
        }
        Some(Formats(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_formats_only() {
        assert_eq!(
            Formats::parse("strict_date_optional_time||epoch_millis"),
            Some(Formats(vec![
                Format::StrictDateOptionalTime,
                Format::EpochMillis
            ]))
        );
        assert_eq!(
            Formats::parse("yyyy/MM/dd HH:mm:ss||yyyy/MM/dd||epoch_millis").map(|f| f.0.len()),
            Some(3)
        );
        assert_eq!(Formats::parse("basic_date"), None);
        assert_eq!(Formats::parse(""), None);
    }
}
