//! A `geo_point` field's value, as `GeoUtils.parseGeoPoint` and
//! `PointParser` read it: an object (`lat`/`lon`, `geohash` or GeoJSON), a
//! string (`"lat,lon"`, a geohash or WKT), an array (`[lon, lat]` or an
//! array of those), then `validate` or, with `ignore_malformed`, `normalize`.

use crate::mapping::json::{Json, java_double};
use crate::mapping::{Leaf, MappingError, Value, java_parse_double};

use super::reader::Tok;
use super::values::FieldValue;
use super::walk::Parser;

const MAX_LAT: f64 = 90.0;
const MAX_LON: f64 = 180.0;

const INVALID_FIELDS: &str = "field must be either [lon|lat], [type|coordinates], or [geohash]";

fn parse_error(reason: &str) -> MappingError {
    MappingError::opensearch_parse(reason)
}

/// `GeoPoint.assertZValue`.
fn assert_z(ignore_z: bool, z: f64) -> Result<(), MappingError> {
    if ignore_z {
        Ok(())
    } else {
        // ALLOC: error path; one message.
        Err(parse_error(&format!(
            "Exception parsing coordinates: found Z value [{}] but [ignore_z_value] parameter is [false]",
            java_double(z)
        )))
    }
}

/// The geohash alphabet, in value order.
const BASE_32: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";

/// `GeoPoint.resetFromGeoHash`: `(lat, lon)` of the cell's lower-left corner, at
/// the 26 bits a dimension `Geohash.mortonEncode` keeps.
///
/// # Errors
///
/// The Java's parse exception, as [`MappingError`] data.
pub fn geohash(text: &str) -> Result<(f64, f64), MappingError> {
    if text.is_empty() {
        return Err(parse_error("empty geohash"));
    }
    let (mut lon_bits, mut lat_bits) = (0u64, 0u64);
    let mut count = 0u32;
    // Characters past the twelfth are ignored; missing ones read as zeros.
    for c in text
        .chars()
        .take(12)
        .map(Some)
        .chain(std::iter::repeat(None))
    {
        if count >= 60 {
            break;
        }
        let v = match c {
            Some(c) => BASE_32
                .iter()
                .position(|b| char::from(*b) == c)
                .ok_or_else(|| {
                    // ALLOC: error path; one message.
                    parse_error(&format!("unsupported symbol [{c}] in geohash [{text}]"))
                })?,
            None => 0,
        };
        let v = u64::try_from(v).unwrap_or(0);
        for bit in (0..5).rev() {
            let b = (v >> bit) & 1;
            // Even bits (from the first) are longitude, odd are latitude.
            if count.is_multiple_of(2) {
                lon_bits = (lon_bits << 1) | b;
            } else {
                lat_bits = (lat_bits << 1) | b;
            }
            count = count.saturating_add(1);
        }
    }
    // Each dimension has 30 bits; Java's morton hash keeps the top 26.
    let scale = f64::from(1u32 << 26);
    #[allow(clippy::cast_precision_loss)]
    let lon = (lon_bits >> 4) as f64 / (scale / 360.0) - 180.0;
    #[allow(clippy::cast_precision_loss)]
    let lat = (lat_bits >> 4) as f64 / (scale / 180.0) - 90.0;
    Ok((lat, lon))
}

/// `GeoPoint.resetFromWKT`: `POINT (lon lat)` or with a z.
fn wkt(text: &str, ignore_z: bool) -> Result<(f64, f64), MappingError> {
    let invalid = || parse_error("Invalid WKT format");
    let rest = text.trim();
    let rest = rest
        .get(..5)
        .filter(|h| h.eq_ignore_ascii_case("point"))
        .and_then(|_| rest.get(5..))
        .ok_or_else(invalid)?
        .trim();
    let inner = rest
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(')'))
        .ok_or_else(invalid)?;
    let nums: Vec<f64> = inner
        .split_whitespace()
        .map(java_parse_double)
        .collect::<Option<_>>()
        .ok_or_else(invalid)?;
    let (x, y) = match nums.as_slice() {
        [x, y] => (*x, *y),
        [x, y, z] => {
            assert_z(ignore_z, *z).map_err(|_| invalid())?;
            (*x, *y)
        }
        _ => return Err(invalid()),
    };
    if !(-MAX_LON..=MAX_LON).contains(&x) || !(-MAX_LAT..=MAX_LAT).contains(&y) {
        return Err(invalid());
    }
    Ok((y, x))
}

/// `GeoPoint.resetFromCoordinates`.
fn coordinates(text: &str, ignore_z: bool) -> Result<(f64, f64), MappingError> {
    let vals: Vec<&str> = text.split(',').collect();
    if vals.len() > 3 {
        // ALLOC: error path; one message.
        return Err(parse_error(&format!(
            "failed to parse [{text}], expected 2 or 3 coordinates but found: [{}]",
            vals.len()
        )));
    }
    let lat = vals
        .first()
        .and_then(|v| java_parse_double(v.trim()))
        .ok_or_else(|| parse_error("latitude must be a number"))?;
    let lon = vals
        .get(1)
        .and_then(|v| java_parse_double(v.trim()))
        .ok_or_else(|| parse_error("longitude must be a number"))?;
    if let Some(z) = vals.get(2) {
        let z = java_parse_double(z.trim()).ok_or_else(|| MappingError::number_format(z.trim()))?;
        assert_z(ignore_z, z)?;
    }
    Ok((lat, lon))
}

/// `GeoPoint.resetFromString`: `(lat, lon)` of a `"lat,lon"`, WKT or geohash text.
///
/// # Errors
///
/// The Java's parse exception, as [`MappingError`] data.
pub fn string_point(text: &str, ignore_z: bool) -> Result<(f64, f64), MappingError> {
    if text.to_lowercase().contains("point") {
        wkt(text, ignore_z)
    } else if text.contains(',') {
        coordinates(text, ignore_z)
    } else {
        geohash(text)
    }
}

fn number(json: &Json) -> Option<f64> {
    match json {
        Json::Num(t) => java_parse_double(t),
        _ => None,
    }
}

/// `parseGeoPointArray`: `[lon, lat]`, a third element the z value.
fn array_point(items: &[Json], ignore_z: bool) -> Result<(f64, f64), MappingError> {
    let mut x = f64::NAN;
    let mut y = f64::NAN;
    for (n, item) in items.iter().enumerate() {
        let v = number(item).ok_or_else(|| parse_error("numeric value expected"))?;
        match n {
            0 => x = v,
            1 => y = v,
            2 => assert_z(ignore_z, v)?,
            _ => {
                return Err(parse_error(
                    "[geo_point] field type does not accept more than 3 values",
                ));
            }
        }
    }
    if items.len() < 2 {
        return Err(parse_error(
            "[geo_point] field type should have at least two dimensions",
        ));
    }
    Ok((y, x))
}

/// An object's `lat` and `lon`.
fn basic_fields(members: &[(String, Json)]) -> Result<(f64, f64), MappingError> {
    let (mut lat, mut lon) = (None, None);
    for (key, value) in members.iter().take(2) {
        if key != "lat" && key != "lon" {
            return Err(parse_error(INVALID_FIELDS));
        }
        let d = match value {
            Json::Num(_) => number(value),
            Json::Str(s) => java_parse_double(s),
            _ => {
                // ALLOC: error path; one message.
                return Err(parse_error(&format!("{key} must be a number")));
            }
        }
        .ok_or_else(|| parse_error("[lon] and [lat] must be valid double values"))?;
        if key == "lat" {
            lat = Some(d);
        } else {
            lon = Some(d);
        }
    }
    let lon = lon.ok_or_else(|| parse_error("field [lon] missing"))?;
    let lat = lat.ok_or_else(|| parse_error("field [lat] missing"))?;
    if members.len() > 2 {
        return Err(parse_error(INVALID_FIELDS));
    }
    Ok((lat, lon))
}

/// An object's GeoJSON `type` and `coordinates`.
fn geo_json(members: &[(String, Json)], ignore_z: bool) -> Result<(f64, f64), MappingError> {
    let (mut typed, mut coords) = (false, None);
    for (key, value) in members.iter().take(2) {
        match (key.as_str(), value) {
            ("type", Json::Str(s)) if s.eq_ignore_ascii_case("point") => typed = true,
            ("type", Json::Str(_)) => return Err(parse_error("type must be Point")),
            ("type", _) => return Err(parse_error("type must be a string")),
            ("coordinates", Json::Arr(items)) => coords = Some(array_point(items, ignore_z)?),
            ("coordinates", _) => return Err(parse_error("coordinates must be an array")),
            _ => return Err(parse_error(INVALID_FIELDS)),
        }
    }
    if !typed {
        return Err(parse_error("field [type] missing"));
    }
    let point = coords.ok_or_else(|| parse_error("field [coordinates] missing"))?;
    if members.len() > 2 {
        return Err(parse_error(INVALID_FIELDS));
    }
    Ok(point)
}

/// `GeoUtils.parseGeoPoint` of one value.
fn parse_point(json: &Json, ignore_z: bool) -> Result<(f64, f64), MappingError> {
    match json {
        Json::Obj(members) => match members.first().map(|(k, _)| k.as_str()) {
            None => Err(parse_error("token [END_OBJECT] not allowed")),
            Some("lat" | "lon") => basic_fields(members),
            Some("geohash") => match members.as_slice() {
                [(_, Json::Str(s))] => geohash(s),
                [(_, _)] => Err(parse_error("geohash must be a string")),
                _ => Err(parse_error(INVALID_FIELDS)),
            },
            Some("type" | "coordinates") => geo_json(members, ignore_z),
            Some(_) => Err(parse_error(INVALID_FIELDS)),
        },
        Json::Arr(items) => array_point(items, ignore_z),
        Json::Str(s) => string_point(s, ignore_z),
        _ => Err(parse_error("geo_point expected")),
    }
}

/// `GeoUtils.centeredModulus`.
fn centered_modulus(dividend: f64, divisor: f64) -> f64 {
    let mut rtn = dividend % divisor;
    if rtn <= 0.0 {
        rtn += divisor;
    }
    if rtn > divisor / 2.0 {
        rtn -= divisor;
    }
    rtn
}

/// `GeoUtils.normalizePoint(point)`.
/// `GeoUtils.normalizePoint`: `(lat, lon)` wrapped into range.
pub fn normalize(lat: f64, lon: f64) -> (f64, f64) {
    let (mut lat, mut lon) = (lat, lon);
    let norm_lat = !(-MAX_LAT..=MAX_LAT).contains(&lat);
    let norm_lon = norm_lat || !(-MAX_LON..=MAX_LON).contains(&lon);
    if norm_lat {
        lat = centered_modulus(lat, 360.0);
        let mut shift = true;
        if lat < -MAX_LAT {
            lat = -180.0 - lat;
        } else if lat > MAX_LAT {
            lat = 180.0 - lat;
        } else {
            shift = false;
        }
        if shift {
            lon += 180.0;
        }
    }
    if norm_lon {
        lon = centered_modulus(lon, 360.0);
    }
    // Avoid -0.0.
    (lat + 0.0, lon + 0.0)
}

/// `PointParser.process`: validate, or normalize under `ignore_malformed`;
/// then the encoding Lucene refuses a NaN with.
fn process(
    name: &str,
    (lat, lon): (f64, f64),
    ignore_malformed: bool,
) -> Result<(f64, f64), MappingError> {
    let (lat, lon) = if ignore_malformed {
        if !lat.is_finite() || !lon.is_finite() {
            return Err(parse_error("cannot normalize the point - not a number"));
        }
        normalize(lat, lon)
    } else {
        if !(-MAX_LAT..=MAX_LAT).contains(&lat) && !lat.is_nan() {
            // ALLOC: error path; one message.
            return Err(MappingError::illegal_argument(&format!(
                "illegal latitude value [{}] for {name}",
                java_double(lat)
            )));
        }
        if !(-MAX_LON..=MAX_LON).contains(&lon) && !lon.is_nan() {
            // ALLOC: error path; one message.
            return Err(MappingError::illegal_argument(&format!(
                "illegal longitude value [{}] for {name}",
                java_double(lon)
            )));
        }
        (lat, lon)
    };
    lucene_index::document::LatLonPoint::encode(lat, lon)
        // ALLOC: error path; one message.
        .map_err(|e| MappingError::illegal_argument(&e.to_string()))?;
    Ok((lat, lon))
}

/// `PointParser.parse` of a whole value: every point it holds.
fn points(
    name: &str,
    json: &Json,
    ignore_z: bool,
    ignore_malformed: bool,
) -> Result<Vec<(f64, f64)>, MappingError> {
    if let Json::Arr(items) = json {
        if matches!(items.first(), Some(Json::Num(_))) {
            let p = array_point_bounded(items, ignore_z)?;
            return Ok(vec![process(name, p, ignore_malformed)?]);
        }
        // ALLOC: one point an element, bounded by the document's bytes.
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(process(
                name,
                parse_point(item, ignore_z)?,
                ignore_malformed,
            )?);
        }
        return Ok(out);
    }
    Ok(vec![process(
        name,
        parse_point(json, ignore_z)?,
        ignore_malformed,
    )?])
}

/// `PointParser`'s own array branch: `[x, y]` or `[x, y, z]`, a fourth value
/// refused as more than three dimensions.
fn array_point_bounded(items: &[Json], ignore_z: bool) -> Result<(f64, f64), MappingError> {
    if items.len() > 3 {
        return Err(parse_error("field type does not accept > 3 dimensions"));
    }
    array_point(items, ignore_z)
}

fn flag(leaf: &Leaf, name: &str) -> bool {
    leaf.get(name).is_some_and(Value::flag)
}

impl Parser<'_, '_> {
    /// A `geo_point` array, after its `[`: `parsesArrayValue` makes the
    /// whole array the field's value.
    pub(super) fn geo_array(&mut self, leaf: &Leaf, parts: &[&str]) -> Result<(), MappingError> {
        let n = self.push(parts);
        let full = self.path_text();
        let read = self.geo_value(leaf, &full, Tok::BeginArray);
        self.pop(n);
        read
    }

    /// A `geo_point` field's value, read whole from `tok`
    /// (`parsesArrayValue` is true, so an array is the field's value).
    pub(super) fn geo_value(
        &mut self,
        leaf: &Leaf,
        full: &str,
        tok: Tok,
    ) -> Result<(), MappingError> {
        let json = self.reader.read_value(tok)?;
        if json == Json::Null {
            return Ok(());
        }
        let simple = full.rsplit('.').next().unwrap_or(full);
        let ignore_malformed = flag(leaf, "ignore_malformed");
        match points(
            simple,
            &json,
            flag(leaf, "ignore_z_value"),
            ignore_malformed,
        ) {
            Ok(found) => {
                for (lat, lon) in found {
                    // ALLOC: one value a point of the document.
                    self.out
                        .fields
                        .push((full.to_owned(), FieldValue::GeoPoint { lat, lon }));
                }
                Ok(())
            }
            Err(_) if ignore_malformed => {
                // Once a field, as `ParseContext`'s set keeps it.
                if !self.out.ignored.iter().any(|f| f == full) {
                    // ALLOC: document parse or error path; bounded by the document's bytes (the HTTP body bound).
                    self.out.ignored.push(full.to_owned());
                }
                Ok(())
            }
            // ALLOC: error path; one message.
            Err(cause) => Err(MappingError::mapper_parsing(&format!(
                "failed to parse field [{full}] of type [geo_point]"
            ))
            .caused_by(cause)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(t: &str) -> Json {
        Json::Num(t.to_owned())
    }

    #[test]
    fn a_geohash_decodes_to_its_lower_left_corner() {
        let (lat, lon) = geohash("u4pruydqqvj").expect("hash");
        assert!((lat - 57.64911).abs() < 1e-4 && (lon - 10.40744).abs() < 1e-4);
        assert!(geohash("").is_err() && geohash("a").is_err());
    }

    #[test]
    fn every_form_reads_as_the_same_point() {
        let want = (41.12, -71.34);
        let obj = Json::Obj(vec![
            ("lat".into(), num("41.12")),
            ("lon".into(), num("-71.34")),
        ]);
        let s = Json::Str("41.12,-71.34".into());
        let arr = Json::Arr(vec![num("-71.34"), num("41.12")]);
        let wkt = Json::Str("POINT (-71.34 41.12)".into());
        let gj = Json::Obj(vec![
            ("type".into(), Json::Str("Point".into())),
            ("coordinates".into(), arr.clone()),
        ]);
        for j in [obj, s, arr, wkt, gj] {
            assert_eq!(parse_point(&j, true).expect("point"), want);
        }
    }

    #[test]
    fn a_point_out_of_range_is_refused_or_normalized() {
        let (lat, lon) = process("p", (91.0, 0.0), true).expect("normalized");
        assert_eq!((lat, lon), (89.0, 180.0));
        let e = process("p", (91.0, 0.0), false).expect_err("refused");
        assert_eq!(e.reason, "illegal latitude value [91.0] for p");
        assert!(process("p", (f64::NAN, 0.0), true).is_err());
    }
}
