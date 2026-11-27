//! The node settings that turn buckets on and size them.

use oss_format::ManifestCodec;
use oss_types::settings::{Empty, Registry, Source};
use oss_writer::bucket::ManifestWrite;
use oss_writer::bucket::settings::{
    ENABLED, MANIFEST_CODEC, MAX_MANIFEST_BYTES, MAX_MEMBERS, SETTINGS, TARGET_BYTES, enabled,
    limits, manifest_write,
};

fn with(pairs: &'static [(&'static str, &'static str)]) -> impl Source {
    pairs
}

#[test]
fn the_defaults_are_the_javas_with_buckets_off() {
    assert!(!enabled(&Empty).unwrap());
    let l = limits(&Empty).unwrap();
    assert_eq!(l.target_bytes(), 1 << 30);
    assert_eq!(l.max_members(), 1024);
    assert_eq!(l.max_manifest_bytes(), 2 << 20);
    assert_eq!(
        manifest_write(&Empty).unwrap(),
        ManifestWrite::Auto(ManifestCodec::Zstd)
    );
}

#[test]
fn each_setting_reads_its_value() {
    let s = with(&[
        ("serverless.buckets.enabled", "true"),
        ("serverless.buckets.max_members", "7"),
        ("serverless.buckets.target_bytes", "4096"),
        ("serverless.buckets.max_manifest_bytes", "512"),
    ]);
    assert!(enabled(&s).unwrap());
    let l = limits(&s).unwrap();
    assert_eq!(l.target_bytes(), 4096);
    assert_eq!(l.max_members(), 7);
    assert_eq!(l.max_manifest_bytes(), 512);
}

#[test]
fn max_members_above_the_ceiling_is_clamped() {
    let s = with(&[("serverless.buckets.max_members", "5000")]);
    assert_eq!(limits(&s).unwrap().max_members(), 1024);
}

#[test]
fn a_size_under_one_or_no_number_is_refused() {
    for (key, value) in [
        ("serverless.buckets.max_members", "0"),
        ("serverless.buckets.target_bytes", "-5"),
        ("serverless.buckets.max_manifest_bytes", "0"),
        ("serverless.buckets.target_bytes", "big"),
    ] {
        let pairs: &'static [(&str, &str)] = Box::leak(Box::new([(key, value)]));
        let err = limits(&pairs).expect_err(key);
        assert!(err.to_string().contains(key), "{err}");
    }
}

#[test]
fn the_codec_names_pick_the_write_form() {
    for (name, want) in [
        ("auto", ManifestWrite::Auto(ManifestCodec::Zstd)),
        ("json", ManifestWrite::Json),
        ("none", ManifestWrite::Compact(ManifestCodec::None)),
        ("deflate", ManifestWrite::Compact(ManifestCodec::Deflate)),
        ("zstd", ManifestWrite::Compact(ManifestCodec::Zstd)),
    ] {
        let pairs: &'static [(&str, &str)] =
            Box::leak(Box::new([("serverless.buckets.manifest_codec", name)]));
        assert_eq!(manifest_write(&pairs).unwrap(), want, "{name}");
    }
}

#[test]
fn an_unknown_or_unwritable_codec_is_refused() {
    for name in ["lz4", "ZSTD", "gzip", ""] {
        let pairs: &'static [(&str, &str)] =
            Box::leak(Box::new([("serverless.buckets.manifest_codec", name)]));
        assert!(manifest_write(&pairs).is_err(), "{name:?}");
    }
}

#[test]
fn the_settings_form_a_valid_registry() {
    let registry = Registry::new(&[SETTINGS]).expect("valid");
    assert_eq!(registry.all().count(), 5);
    assert!(!ENABLED.default());
    assert_eq!(MAX_MEMBERS.default(), 1024);
    assert_eq!(TARGET_BYTES.default(), 1 << 30);
    assert_eq!(MAX_MANIFEST_BYTES.default(), 2 << 20);
    assert_eq!(MANIFEST_CODEC.default(), "auto");
}
