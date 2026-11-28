//! docs/specs/geospatial AC-1 and AC-11: `geo_point` in a mapping and as a
//! Lucene field. Expected points come from the encoding's definition
//! (`GeoEncodingUtils`: `floor(degrees / (range / 2^32))`), not from the
//! code under test; no stock-server recordings exist yet.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_index::index_writer::IndexWriter;
use lucene_index::segment_info::LuceneVersion;
use lucene_search::directory_reader::DirectoryReader;
use lucene_search::reader::LeafReader;
use lucene_store::ByteBuffersDirectory;
use oss_doc::document;
use oss_doc::lucene::{Indexer, Meta};
use oss_doc::mapping::{Mapping, ParseOptions};

const VERSION: LuceneVersion = LuceneVersion {
    major: 10,
    minor: 4,
    bugfix: 0,
};

const ON: ParseOptions = ParseOptions {
    knn: None,
    nested: false,
    geo: true,
};

const MAPPING: &str = r#"{"properties":{"loc":{"type":"geo_point"},"lax":{"type":"geo_point","ignore_malformed":true}}}"#;

/// `LatLonDocValuesField`'s packed value of a point, from the definition.
fn packed(lat: f64, lon: f64) -> i64 {
    let lat_enc = (lat / (180.0 / 4_294_967_296.0)).floor() as i32;
    let lon_enc = (lon / (360.0 / 4_294_967_296.0)).floor() as i32;
    (i64::from(lat_enc) << 32) | (i64::from(lon_enc) & 0xFFFF_FFFF)
}

fn index(source: &str) -> Result<ByteBuffersDirectory, String> {
    let m = Mapping::parse_with(MAPPING.as_bytes(), ON).expect("mapping");
    let parsed = document::parse(&m, "1", source.as_bytes()).map_err(|e| e.to_string())?;
    let dir = ByteBuffersDirectory::new();
    {
        let mut w = IndexWriter::open(&dir, Vec::new(), "Lucene104", VERSION).expect("writer");
        w.enable_explicit_documents().expect("explicit");
        let mut indexer = Indexer::new();
        let meta = Meta {
            id: "1",
            seq_no: 0,
            primary_term: 1,
            version: 1,
            source: source.as_bytes(),
        };
        let doc = indexer.document(&mut w, &m, &parsed, &meta).expect("doc");
        w.add_explicit_documents(vec![doc]).expect("add");
        w.commit().expect("commit");
    }
    Ok(dir)
}

fn values(dir: &ByteBuffersDirectory, field: &str) -> Vec<i64> {
    let reader = DirectoryReader::open(dir).expect("open");
    let segment = reader.segment_readers().first().expect("a segment");
    let Some(mut dv) = segment.sorted_numeric_doc_values(field).expect("reads") else {
        return Vec::new();
    };
    if !dv.advance_exact(0).expect("advances") {
        return Vec::new();
    }
    (0..dv.doc_value_count())
        .map(|_| dv.next_value().expect("a value"))
        .collect()
}

#[test]
fn every_input_form_indexes_to_the_same_encoded_point() {
    let want = packed(41.12, -71.34);
    for form in [
        r#"{"lat":41.12,"lon":-71.34}"#,
        r#"{"lat":"41.12","lon":"-71.34"}"#,
        r#""41.12,-71.34""#,
        r#""41.12, -71.34""#,
        r#"[-71.34,41.12]"#,
        r#"[-71.34,41.12,100]"#,
        r#""POINT (-71.34 41.12)""#,
        r#""point(-71.34 41.12)""#,
        r#"{"type":"Point","coordinates":[-71.34,41.12]}"#,
    ] {
        let dir = index(&format!(r#"{{"loc":{form}}}"#)).unwrap_or_else(|e| panic!("{form}: {e}"));
        assert_eq!(values(&dir, "loc"), [want], "{form}");
    }
}

#[test]
fn a_geohash_indexes_to_its_cell_corner() {
    let dir = index(r#"{"loc":"u4pruydqqvj"}"#).expect("indexes");
    let got = values(&dir, "loc");
    // The cell of an 11-character hash is under a metre wide.
    let near = packed(57.64911, 10.40744);
    assert_eq!(got.len(), 1);
    assert!((got[0] >> 32).abs_diff(near >> 32) < 20_000);
}

#[test]
fn an_array_of_points_keeps_each_point_and_a_point_is_a_two_dimension_point() {
    let dir = index(r#"{"loc":[{"lat":10,"lon":20},"30,40",[50,60]]}"#).expect("indexes");
    let mut want = vec![packed(10.0, 20.0), packed(30.0, 40.0), packed(60.0, 50.0)];
    want.sort_unstable();
    assert_eq!(values(&dir, "loc"), want);
    let reader = DirectoryReader::open(&dir).expect("open");
    let segment = reader.segment_readers().first().expect("a segment");
    let info = segment.field_infos().field_by_name("loc").expect("field");
    assert_eq!((info.point_dimension_count, info.point_num_bytes), (2, 4));
}

#[test]
fn an_empty_array_and_a_null_add_no_value() {
    assert!(values(&index(r#"{"loc":[]}"#).expect("indexes"), "loc").is_empty());
    assert!(values(&index(r#"{"loc":null}"#).expect("indexes"), "loc").is_empty());
}

#[test]
fn an_invalid_value_is_refused_naming_the_field_unless_ignore_malformed() {
    let e = index(r#"{"loc":{"lat":91,"lon":0}}"#).expect_err("refused");
    assert!(
        e.contains("failed to parse field [loc] of type [geo_point]"),
        "{e}"
    );
    let e = index(r#"{"loc":{"lat":0,"lon":181}}"#).expect_err("refused");
    assert!(
        e.contains("failed to parse field [loc] of type [geo_point]"),
        "{e}"
    );
    for bad in [
        r#""abc,1""#,
        r#"{"lat":1}"#,
        r#"[1]"#,
        r#"[1,2,3,4]"#,
        "true",
        r#""POINT EMPTY""#,
    ] {
        assert!(index(&format!(r#"{{"loc":{bad}}}"#)).is_err(), "{bad}");
    }
    // `ignore_malformed` wraps an out-of-range point and drops what it cannot read.
    let dir = index(r#"{"lax":{"lat":91,"lon":0}}"#).expect("normalized");
    assert_eq!(values(&dir, "lax"), [packed(89.0, 180.0)]);
    let dir = index(r#"{"lax":"nonsense,1"}"#).expect("ignored");
    assert!(values(&dir, "lax").is_empty());
}

#[test]
fn the_field_maps_only_where_the_node_indexes_it_and_round_trips() {
    let off = Mapping::parse(MAPPING.as_bytes()).expect_err("unsupported");
    assert!(off.is_unsupported());
    let m = Mapping::from_descriptor(Some(
        r#"{"properties":{"loc":{"type":"geo_point","doc_values":false,"ignore_z_value":false,"meta":{"a":"b"}}}}"#,
    ))
    .expect("maps");
    assert_eq!(
        m.to_json(),
        r#"{"properties":{"loc":{"type":"geo_point","doc_values":false,"meta":{"a":"b"},"ignore_z_value":false}}}"#
    );
    for refused in [
        r#"{"properties":{"p":{"type":"geo_point","null_value":"1,2"}}}"#,
        r#"{"properties":{"p":{"type":"geo_point","fields":{"k":{"type":"keyword"}}}}}"#,
        r#"{"properties":{"p":{"type":"geo_point","copy_to":"q"}}}"#,
    ] {
        let e = Mapping::from_descriptor(Some(refused)).expect_err(refused);
        assert!(e.is_unsupported(), "{refused}");
    }
}
