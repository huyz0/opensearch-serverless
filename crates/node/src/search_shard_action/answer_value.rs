//! A shard's result for one aggregation as the forwarded answer's value.

use super::{histogram, multi, range, rare, refuse, terms};
use oss_reader::aggs::partial::{Format, Value as Part};
use oss_transport::actions::search_shard::{AggRequest, AggValue};
use oss_transport::exception::RemoteException;

/// The request named `name`, which a bucket answer repeats the options of.
pub(super) fn request<'r>(
    asked: &'r [AggRequest],
    name: &str,
) -> Result<&'r AggRequest, RemoteException> {
    asked
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| refuse("an answer without its request", "INTERNAL_SERVER_ERROR"))
}

/// The wire's value for `v`, the result of the aggregation `name` among
/// `asked`, of a field whose format is `format`.
///
/// # Errors
///
/// [`RemoteException`] for a result of a kind this node does not answer.
pub(super) fn value(
    name: &str,
    v: &Part,
    format: Format,
    asked: &[AggRequest],
) -> Result<AggValue, RemoteException> {
    Ok(match v {
        Part::Min(v) => AggValue::Min(*v),
        Part::Max(v) => AggValue::Max(*v),
        Part::Sum(v) => AggValue::Sum(*v),
        Part::Avg { sum, count } => AggValue::Avg {
            sum: *sum,
            count: i64::try_from(*count).unwrap_or(i64::MAX),
        },
        Part::Stats {
            count,
            sum,
            min,
            max,
        } => AggValue::Stats {
            count: i64::try_from(*count).unwrap_or(i64::MAX),
            min: *min,
            max: *max,
            sum: *sum,
        },
        Part::Cardinality(sketch) => AggValue::Cardinality(
            sketch
                .as_ref()
                .map(oss_reader::aggs::metric::hll::Hll::to_bytes),
        ),
        Part::Percentiles {
            ranks,
            keys,
            keyed,
            compression,
            digest,
        } => {
            use oss_reader::aggs::metric::tdigest::Digest;
            AggValue::Percentiles {
                ranks: *ranks,
                keys: keys.clone(),
                keyed: *keyed,
                digest: digest
                    .as_ref()
                    .map_or_else(|| Digest::new(*compression).to_bytes(), Digest::to_bytes),
            }
        }
        Part::Terms(r) => AggValue::Terms(terms::numeric(r, request(asked, name)?, format)?),
        Part::KeywordTerms(r) => AggValue::Terms(terms::keyword(r, request(asked, name)?)?),
        Part::DateHistogram(r) => {
            AggValue::DateHistogram(histogram::answer(r, request(asked, name)?)?)
        }
        Part::Range(r) => AggValue::Range(range::answer(r, format, request(asked, name)?)?),
        Part::Rare(r) => AggValue::Rare(rare::answer(r)?),
        Part::Multi(r) => AggValue::Multi(multi::answer(r, request(asked, name)?)?),
        Part::GeoCentroid { count, lat, lon } => AggValue::GeoCentroid {
            count: i64::try_from(*count).unwrap_or(i64::MAX),
            lat: *lat,
            lon: *lon,
        },
        Part::GeoBounds {
            top,
            bottom,
            pos_left,
            pos_right,
            neg_left,
            neg_right,
            wrap,
        } => AggValue::GeoBounds {
            top: *top,
            bottom: *bottom,
            pos_left: *pos_left,
            pos_right: *pos_right,
            neg_left: *neg_left,
            neg_right: *neg_right,
            wrap: *wrap,
        },
        Part::ValueCount(n) => AggValue::ValueCount(i64::try_from(*n).unwrap_or(i64::MAX)),
        _ => {
            return Err(refuse(
                "a forwarded aggregation of this kind is not supported by this node",
                "BAD_REQUEST",
            ));
        }
    })
}
