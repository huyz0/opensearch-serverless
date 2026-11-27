//! A range's `time_zone`: a fixed offset or an IANA region, and how a wall-clock time in it
//! becomes an instant.
//!
//! Ported from `DateUtils.of`/`ZoneId.of` and `ZonedDateTime.ofLocal` (server/src/main/java/org/opensearch/common/time/DateUtils.java) at OpenSearch 3.10.0.

use crate::aggs::bucket::date_histogram::RegionZone;
use crate::date::{DateSpec, parse_zone};

/// One day, the span either side of a wall-clock time within which an offset change is looked for.
const DAY_MS: i64 = 86_400_000;

/// A `time_zone` as the range reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeZone {
    /// Seconds east of UTC of a fixed zone; zero for a region.
    pub fixed: i64,
    /// The region, when the id names one.
    pub region: Option<RegionZone>,
}

/// The zone `text` names: a fixed one ([`parse_zone`]) or a region spelled as the tz database
/// spells it; `None` for anything else.
#[must_use]
pub fn parse_time_zone(text: &str) -> Option<TimeZone> {
    if let Some(fixed) = parse_zone(text) {
        return Some(TimeZone {
            fixed,
            region: None,
        });
    }
    RegionZone::named(text).map(|region| TimeZone {
        fixed: 0,
        region: Some(region),
    })
}

impl DateSpec {
    /// The zone's offset at the instant `utc`, in milliseconds.
    pub(crate) fn offset_at(&self, utc: i64) -> i64 {
        match self.region {
            Some(region) => region.offset_ms(utc),
            // ARITH: a zone is at most 18 hours.
            None => self.zone.saturating_mul(1_000),
        }
    }

    /// The instant a wall-clock time (as milliseconds since the epoch, read as if UTC) is in the
    /// zone. In an overlap the earlier offset applies, and a time in a gap is shifted forward by
    /// the gap, as `ZonedDateTime.ofLocal` does.
    pub(crate) fn utc_of_local(&self, local: i64) -> i64 {
        let Some(region) = self.region else {
            return local.saturating_sub(self.offset_at(0));
        };
        let before = region.offset_ms(local.saturating_sub(DAY_MS));
        let after = region.offset_ms(local.saturating_add(DAY_MS));
        let holds = |offset: i64| region.offset_ms(local.saturating_sub(offset)) == offset;
        let offset = if holds(before) {
            before
        } else if holds(after) {
            after
        } else {
            before
        };
        local.saturating_sub(offset)
    }
}

#[cfg(test)]
#[path = "date_zone_tests.rs"]
mod tests;
