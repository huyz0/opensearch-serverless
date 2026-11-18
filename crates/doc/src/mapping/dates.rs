//! A `date` field's `format`: the `||`-separated formats this writer reads.
//!
//! `DateFormatter.forPattern` takes any of OpenSearch's built-in names and
//! any `java.time` pattern. This writer takes the default
//! (`strict_date_optional_time||epoch_millis`), the formats dynamic mapping
//! gives a date (`yyyy/MM/dd HH:mm:ss`, `yyyy/MM/dd`), and the epoch
//! formats; a field with any other format is refused as unsupported, so its
//! index stays with a Java writer.

use super::date_text::{digits, eat, epoch, iso, month_len, slash};

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

impl Format {
    /// Milliseconds since the epoch for `text`, or `None` when this format
    /// does not read it. Sub-millisecond digits are dropped, as a
    /// millisecond-resolution `date` field drops them.
    pub fn parse_millis(self, text: &str) -> Option<i64> {
        match self {
            Format::StrictDateOptionalTime => iso(text, true),
            Format::DateOptionalTime => iso(text, false),
            Format::EpochMillis => epoch(text, 6, 1),
            Format::EpochSecond => epoch(text, 9, 1000),
            Format::SlashDateTime => slash(text, true),
            Format::SlashDate => slash(text, false),
        }
    }
}

impl Formats {
    /// Milliseconds since the epoch from the first format that reads
    /// `text`, as `DateFormatter.parse` tries a `||` list.
    pub fn parse_millis(&self, text: &str) -> Option<i64> {
        self.0.iter().find_map(|f| f.parse_millis(text))
    }
}

impl Formats {
    /// `LocalDate.of`'s message when a pattern format reads `text` with a
    /// day the month does not have (`2024/02/30`): the pattern formats
    /// resolve the date after parsing, so this stops the format list.
    pub fn invalid_day(&self, text: &str) -> Option<String> {
        let first = self.0.iter().find(|f| {
            matches!(f, Format::SlashDate | Format::SlashDateTime)
                && slash_fields(text, matches!(f, Format::SlashDateTime)).is_some()
        })?;
        let time = matches!(first, Format::SlashDateTime);
        let (year, month, day) = slash_fields(text, time)?;
        if day <= month_len(year, month) {
            return None;
        }
        const MONTHS: [&str; 12] = [
            "JANUARY",
            "FEBRUARY",
            "MARCH",
            "APRIL",
            "MAY",
            "JUNE",
            "JULY",
            "AUGUST",
            "SEPTEMBER",
            "OCTOBER",
            "NOVEMBER",
            "DECEMBER",
        ];
        let name = MONTHS.get(usize::try_from(month.saturating_sub(1)).ok()?)?;
        Some(if month == 2 && day == 29 {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            format!("Invalid date 'February 29' as '{year}' is not a leap year")
        } else {
            // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
            format!("Invalid date '{name} {day}'")
        })
    }
}

/// The year, month and day of a pattern date whose fields are each in
/// range (month 1 to 12, day 1 to 31), before the day is checked
/// against the month.
fn slash_fields(text: &str, time: bool) -> Option<(i64, i64, i64)> {
    let mut s = text.as_bytes();
    let year = digits(&mut s, 4, 9)?;
    let month = if eat(&mut s, b'/') {
        digits(&mut s, 2, 2)?
    } else {
        return None;
    };
    let day = if eat(&mut s, b'/') {
        digits(&mut s, 2, 2)?
    } else {
        return None;
    };
    if time {
        if !eat(&mut s, b' ') {
            return None;
        }
        for (i, sep) in [(0, b':'), (1, b':'), (2, 0)] {
            let v = digits(&mut s, 2, 2)?;
            if v > [23, 59, 59][i] || (sep != 0 && !eat(&mut s, sep)) {
                return None;
            }
        }
    }
    (s.is_empty() && (1..=12).contains(&month) && (1..=31).contains(&day))
        .then_some((year, month, day))
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

    #[test]
    fn millis() {
        let iso = Format::StrictDateOptionalTime;
        assert_eq!(iso.parse_millis("1970-01-01"), Some(0));
        assert_eq!(
            iso.parse_millis("2024-01-02T03:04:05.123+01:00"),
            Some(1_704_161_045_123)
        );
        assert_eq!(
            iso.parse_millis("2024-02-03T04:05:06.123456789Z"),
            Some(1_706_933_106_123)
        );
        assert_eq!(
            iso.parse_millis("2024-02-03T04:05:06+0530"),
            Some(1_706_913_306_000)
        );
        assert_eq!(iso.parse_millis("2024-02-03T04"), Some(1_706_932_800_000));
        assert_eq!(iso.parse_millis("2024"), Some(1_704_067_200_000));
        assert_eq!(iso.parse_millis("2024-02-29"), Some(1_709_164_800_000));
        assert_eq!(iso.parse_millis("1969-12-31T23:59:59.999Z"), Some(-1));
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "24-01-01",
            "2024-1-01",
            "2024-01-02 03:04",
            "2024-01-02T25",
            "2024-01-02T03:04:05+19",
            "yesterday",
            "2024-01-02T03.5",
        ] {
            assert_eq!(iso.parse_millis(bad), None, "{bad}");
        }
        assert_eq!(
            Format::DateOptionalTime.parse_millis("2024-1-2"),
            Some(1_704_153_600_000)
        );
        let ms = Format::EpochMillis;
        assert_eq!(ms.parse_millis("1700000000000"), Some(1_700_000_000_000));
        assert_eq!(ms.parse_millis("-1000"), Some(-1000));
        assert_eq!(ms.parse_millis("12.5"), Some(12));
        assert_eq!(ms.parse_millis("12.1234567"), None);
        assert_eq!(ms.parse_millis("1e3"), None);
        let sec = Format::EpochSecond;
        assert_eq!(sec.parse_millis("1700000000.5"), Some(1_700_000_000_500));
        assert_eq!(sec.parse_millis("-2"), Some(-2000));
        assert_eq!(sec.parse_millis("1."), None);
        assert_eq!(
            Format::SlashDate.parse_millis("2024/01/02"),
            Some(1_704_153_600_000)
        );
        assert_eq!(
            Format::SlashDateTime.parse_millis("2024/01/02 03:04:05"),
            Some(1_704_164_645_000)
        );
        for bad in [
            "2024-01-02",
            "2024/01",
            "2024/01-02",
            "2024/01/02 03",
            "2024/01/02 03:04",
            "2024/01/02 03:04-05",
            "2024/01/02x",
        ] {
            assert_eq!(Format::SlashDateTime.parse_millis(bad), None, "{bad}");
            assert_eq!(Format::SlashDate.parse_millis(bad), None, "{bad}");
        }
        let f = Formats::parse("yyyy/MM/dd||epoch_millis").unwrap();
        assert_eq!(
            f.invalid_day("2024/02/30").as_deref(),
            Some("Invalid date 'FEBRUARY 30'")
        );
        assert_eq!(
            f.invalid_day("2023/02/29").as_deref(),
            Some("Invalid date 'February 29' as '2023' is not a leap year")
        );
        assert_eq!(f.invalid_day("2024/02/29"), None);
        assert_eq!(f.invalid_day("2024/13/01"), None);
        assert_eq!(f.invalid_day("2024/01/32"), None);
        let t = Formats::parse("yyyy/MM/dd HH:mm:ss").unwrap();
        assert_eq!(
            t.invalid_day("2024/04/31 01:02:03").as_deref(),
            Some("Invalid date 'APRIL 31'")
        );
        assert_eq!(t.invalid_day("2024/04/31 24:02:03"), None);
        assert_eq!(
            Formats::parse("epoch_millis")
                .unwrap()
                .invalid_day("2024/02/30"),
            None
        );
        assert_eq!(f.parse_millis("1700000000000"), Some(1_700_000_000_000));
        assert_eq!(f.parse_millis("x"), None);
    }
}
