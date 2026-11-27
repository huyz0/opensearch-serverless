# A range's `format` and fixed `time_zone`

## Serves

The "a range's `format`, `time_zone` and `relation`" refusal of [compatibility](../../compatibility.md). Kibana and the official clients send `"format":"strict_date_optional_time"` and often a `time_zone` on the range of every time filter, and each answered 400.

## Change

- `format` of `strict_date_optional_time`, `epoch_millis` or `strict_date_optional_time||epoch_millis` (the default) on a date field's range forces the formats the bounds may be written in: `epoch_millis` alone reads only epoch milliseconds, `strict_date_optional_time` alone reads only the dates. Other formats (`yyyy-MM-dd`, `date_optional_time`) stay refused.
- `time_zone` of `Z`, `UTC`, `GMT`, `UT` or a fixed offset (`+HH`, `+HH:mm`, `+HHmm`, up to 18 hours) is the zone a bound with no zone of its own is read in. A bound with an offset keeps it, an epoch-millisecond bound is an instant already, and date math (`now/d`, `-1d/d`, `anchor||/d`) rounds and adds on the zone's calendar, as `JavaDateMathParser` does with the zone. A region (`Asia/Kolkata`) stays refused.
- `format` and `time_zone` on a field that is not a date are refused by name, not guessed.
- A bound that a forced format cannot read is refused as "not supported by this node yet"; the exception OpenSearch answers for it is not recorded, and the one for the default format is.
- Not here: `relation` and `_name`, which stay refused.

## Acceptance criteria

- AC-1: `lte` of a wall-clock time with `time_zone` `+01:00` matches what the same instant written with `Z` matches, a bound with its own offset ignores the range's zone, and `UTC` is the unzoned reading.
- AC-2: `now/d` and `anchor||/d` round to the start of the zone's day.
- AC-3: `epoch_millis` and `strict_date_optional_time` read the same instant as the default; text the forced format cannot read, a region zone and another format answer 400 "not supported by this node yet".

## S3 request budget

None added: only how a bound is read changes.

## Switch

None: refusals become answers, and a range without them reads as before.

## Tests

The reader's `date_math` and `query` unit tests, and `crates/node/tests/it/search_range_zone.rs` through HTTP over the compat index whose `published` field is a date.
