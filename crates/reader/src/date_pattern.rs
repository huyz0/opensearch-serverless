//! A date printed by a `format`: `DateFormatter.forPattern(format).format` for
//! the names and `java.time` patterns this node prints, at OpenSearch 3.10.0.
//!
//! A pattern is read once into parts. The letters served are `y` and `u`
//! (the year: one letter unpadded, two the last two digits, four or more
//! padded and signed past 9999), `M`, `d`, `H`, `m` and `s` (one letter
//! unpadded, two padded), `S` (a fraction of that many digits) and quoted or
//! punctuation literals; a letter outside them, a zone letter included, is
//! not served. The names are `epoch_millis` and the date and time names in
//! [`named`]. The text is of the local time in the zone's offset.

use std::io::{self, Write};

use crate::aggs::metric::date_text::civil_from_days;

const MILLIS_PER_DAY: i64 = 86_400_000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Literal(String),
    /// A year: the letter count, and whether `y` (year of era).
    Year(u8, bool),
    Month(u8),
    Day(u8),
    Hour(u8),
    Minute(u8),
    Second(u8),
    Fraction(u8),
    /// `epoch_millis`: the instant, not the local time.
    EpochMillis,
}

/// A date format this node prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatePattern {
    parts: Vec<Part>,
    /// A year past 9999 or before 0 is an error, as a four-digit
    /// `DateTimeFormatter` builder with `NOT_NEGATIVE` makes it.
    fixed_year: bool,
}

/// The pattern a built-in name stands for, and whether its year is fixed.
fn named(name: &str) -> Option<(&'static str, bool)> {
    Some(match name {
        "strict_date" | "date" => ("yyyy-MM-dd", false),
        "basic_date" => ("yyyyMMdd", true),
        "strict_year_month" | "year_month" => ("yyyy-MM", false),
        "strict_year" | "year" => ("yyyy", false),
        "strict_hour_minute_second" | "hour_minute_second" => ("HH:mm:ss", false),
        _ => return None,
    })
}

impl DatePattern {
    /// Reads `format`, or `None` for a name or letter this node does not
    /// print.
    pub fn parse(format: &str) -> Option<Self> {
        if format == "epoch_millis" {
            return Some(Self {
                // ALLOC: one part.
                parts: vec![Part::EpochMillis],
                fixed_year: false,
            });
        }
        let (pattern, fixed_year) = named(format).unwrap_or((format, false));
        // ALLOC: the format's characters, read once with the request.
        let chars: Vec<char> = pattern.chars().collect();
        // ALLOC: one part a letter run, at most the format's length.
        let mut parts = Vec::new();
        let mut i = 0;
        while let Some(&c) = chars.get(i) {
            if c == '\'' {
                // ALLOC: one literal, at most the format's length.
                let mut text = String::new();
                i = i.saturating_add(1);
                loop {
                    match chars.get(i) {
                        None => return None,
                        Some('\'') if chars.get(i.saturating_add(1)) == Some(&'\'') => {
                            text.push('\'');
                            i = i.saturating_add(2);
                        }
                        Some('\'') => {
                            i = i.saturating_add(1);
                            break;
                        }
                        Some(&other) => {
                            text.push(other);
                            i = i.saturating_add(1);
                        }
                    }
                }
                parts.push(Part::Literal(if text.is_empty() {
                    "'".into()
                } else {
                    text
                }));
                continue;
            }
            if !c.is_ascii_alphabetic() {
                if matches!(c, '#' | '{' | '}' | '[' | ']') {
                    return None;
                }
                // ALLOC: one literal, at most the format's length.
                parts.push(Part::Literal(c.to_string()));
                i = i.saturating_add(1);
                continue;
            }
            let mut n = 0u8;
            while chars.get(i) == Some(&c) {
                n = n.saturating_add(1);
                i = i.saturating_add(1);
            }
            parts.push(match (c, n) {
                ('y' | 'u', 1 | 2 | 4..=9) => Part::Year(n, c == 'y'),
                ('M', 1 | 2) => Part::Month(n),
                ('d', 1 | 2) => Part::Day(n),
                ('H', 1 | 2) => Part::Hour(n),
                ('m', 1 | 2) => Part::Minute(n),
                ('s', 1 | 2) => Part::Second(n),
                ('S', 1..=9) => Part::Fraction(n),
                _ => return None,
            });
        }
        (!parts.is_empty()).then_some(Self { parts, fixed_year })
    }

    /// Writes the instant `key` in the zone whose offset is `zone_ms`.
    ///
    /// # Errors
    ///
    /// An I/O error of `out`, or one for a year a fixed-year format cannot
    /// hold.
    pub fn write(&self, out: &mut dyn Write, key: i64, zone_ms: i64) -> io::Result<()> {
        let local = key.saturating_add(zone_ms);
        let days = local.div_euclid(MILLIS_PER_DAY);
        let in_day = local.rem_euclid(MILLIS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        if self.fixed_year && !(0..=9999).contains(&year) {
            return Err(io::Error::other("the year is outside the format's range"));
        }
        for part in &self.parts {
            match part {
                Part::Literal(text) => out.write_all(text.as_bytes())?,
                Part::EpochMillis => write!(out, "{key}")?,
                Part::Year(n, era) => write_year(out, year, *n, *era)?,
                Part::Month(n) => number(out, i64::from(month), *n)?,
                Part::Day(n) => number(out, i64::from(day), *n)?,
                Part::Hour(n) => number(out, in_day / 3_600_000, *n)?,
                Part::Minute(n) => number(out, in_day / 60_000 % 60, *n)?,
                Part::Second(n) => number(out, in_day / 1_000 % 60, *n)?,
                Part::Fraction(n) => {
                    // ALLOC: one fraction text, 9 bytes.
                    let digits = format!("{:03}000000", in_day % 1_000);
                    out.write_all(digits.get(..usize::from(*n)).unwrap_or("").as_bytes())?;
                }
            }
        }
        Ok(())
    }
}

fn number(out: &mut dyn Write, value: i64, letters: u8) -> io::Result<()> {
    if letters == 2 {
        write!(out, "{value:02}")
    } else {
        write!(out, "{value}")
    }
}

fn write_year(out: &mut dyn Write, year: i64, letters: u8, era: bool) -> io::Result<()> {
    // `y` is the year of the era: a year before 1 counts back from 1.
    let year = if era && year < 1 {
        1i64.saturating_sub(year)
    } else {
        year
    };
    match letters {
        1 => write!(out, "{year}"),
        2 => write!(out, "{:02}", year.rem_euclid(100)),
        n => {
            let width = usize::from(n);
            if year > 9999 && width == 4 {
                write!(out, "+{year}")
            } else if year < 0 {
                write!(out, "-{:0width$}", year.unsigned_abs())
            } else {
                write!(out, "{year:0width$}")
            }
        }
    }
}

#[cfg(test)]
#[path = "date_pattern_tests.rs"]
mod tests;
