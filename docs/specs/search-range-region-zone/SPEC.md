# A range's region `time_zone`

## Serves

The region half of the "a range's `format`, `time_zone` and `relation`" refusal of [compatibility](../../compatibility.md). Kibana sends the browser's zone (`Europe/Paris`, `Asia/Kolkata`) on the range of every time filter, and a region zone answered 400 while a fixed offset was read ([search-range-format-zone](../search-range-format-zone/SPEC.md)).

## Change

- `time_zone` may be an IANA region id as the tz database spells it (`Europe/Paris`; `ZoneId.of` is case-sensitive). It is read with the zone rules the `date_histogram` already holds, so no new data is added.
- A bound with no zone of its own is read as wall-clock time in the region: the offset in force at that wall-clock time applies, an overlap (clocks set back) takes the earlier offset, and a gap (clocks set forward) shifts the time forward by the gap, as `ZonedDateTime.ofLocal` does.
- Date math (`now/d`, `-1d/d`, `anchor||/d`) rounds and adds on the region's calendar: the instant is shifted to wall-clock time by the offset at that instant, the operations run, and the result is read back as above, so `now/d` is the start of the region's day even on a day of 23 or 25 hours.
- A bound with its own offset and an epoch-millisecond bound are instants already and ignore the zone.
- An id that is neither a fixed zone nor a region stays refused as "not supported by this node yet".

## Acceptance criteria

- AC-1: a range with `time_zone` `Europe/Paris` reads a wall-clock bound as the same instant a fixed `+01:00` (winter) or `+02:00` (summer) reads it, and a bound with its own offset ignores the zone.
- AC-2: `anchor||/d` and `+1d` in a region follow the day's length: across the spring-forward day the next midnight is 23 hours after the previous one, across fall-back 25.
- AC-3: a wall-clock time inside a gap reads as shifted forward by the gap, and one inside an overlap reads as the earlier instant.
- AC-4: an unknown id (`Mars/Base`, a wrong-case `europe/paris`) answers 400 "not supported by this node yet".

## S3 request budget

None added: only how a bound is read changes.

## Switch

None: refusals become answers, and a range without a region reads as before.

## Tests

The reader's `date_math` and `query` unit tests, and `crates/node/tests/it/search_range_zone.rs` through HTTP.
