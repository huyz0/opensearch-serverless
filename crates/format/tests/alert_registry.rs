//! docs/specs/alerting/SPEC.md, slice 1: the registry shard and run claim
//! formats.

use oss_format::{
    FormatError, MAX_REGISTRY_ENTRIES, MAX_REGISTRY_SHARD_BYTES, MAX_TENANT_ID_BYTES,
    REGISTRY_SHARDS, RegistryShard, check_tenant_id, decode_run_claim, encode_run_claim,
    registry_shard_key, registry_shard_of, run_claim_key,
};

fn written(shard: &RegistryShard) -> Vec<u8> {
    let mut buf = vec![0u8; MAX_REGISTRY_SHARD_BYTES];
    let n = shard.write(&mut buf).unwrap();
    buf.truncate(n);
    buf
}

#[test]
fn a_shard_keeps_its_tenants_in_byte_order_and_reads_back_what_it_wrote() {
    let mut shard = RegistryShard::new();
    assert!(shard.insert("zeta").unwrap());
    assert!(shard.insert("alpha").unwrap());
    assert!(shard.insert("Mid").unwrap());
    assert!(!shard.insert("alpha").unwrap());
    assert_eq!(written(&shard), b"Mid\nalpha\nzeta\n");
    let back = RegistryShard::parse(&written(&shard)).unwrap();
    assert_eq!(back, shard);
    assert_eq!(back.len(), 3);
    assert!(back.contains("alpha") && !back.contains("beta"));
    assert_eq!(back.tenants().collect::<Vec<_>>(), ["Mid", "alpha", "zeta"]);
}

#[test]
fn an_empty_shard_is_no_bytes_and_removal_reports_whether_it_was_there() {
    let mut shard = RegistryShard::new();
    assert!(shard.is_empty());
    assert_eq!(written(&shard), b"");
    assert_eq!(RegistryShard::parse(b"").unwrap(), shard);
    shard.insert("a").unwrap();
    assert!(!shard.is_empty());
    assert!(shard.remove("a"));
    assert!(!shard.remove("a"));
    assert!(shard.is_empty());
}

#[test]
fn a_shard_past_its_entry_bound_refuses_a_new_tenant_but_keeps_a_present_one() {
    let mut shard = RegistryShard::new();
    for i in 0..MAX_REGISTRY_ENTRIES {
        shard.insert(&format!("t{i:05}")).unwrap();
    }
    assert!(matches!(
        shard.insert("one-more"),
        Err(FormatError::TooLarge { .. })
    ));
    assert!(!shard.insert("t00007").unwrap());
    assert_eq!(shard.len(), MAX_REGISTRY_ENTRIES);
    let bytes = written(&shard);
    assert!(bytes.len() <= MAX_REGISTRY_SHARD_BYTES);
    assert_eq!(RegistryShard::parse(&bytes).unwrap(), shard);
}

#[test]
fn a_damaged_shard_is_refused_never_taken() {
    for bad in [
        &b"b\na\n"[..],
        b"a\na\n",
        b"a\n\nb\n",
        b"a",
        b"a/b\n",
        b"\n",
    ] {
        assert!(RegistryShard::parse(bad).is_err(), "{bad:?}");
    }
    let mut many = Vec::new();
    for i in 0..=MAX_REGISTRY_ENTRIES {
        many.extend_from_slice(format!("t{i:05}\n").as_bytes());
    }
    assert!(matches!(
        RegistryShard::parse(&many),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_write_into_a_short_buffer_is_refused() {
    let mut shard = RegistryShard::new();
    shard.insert("tenant").unwrap();
    assert!(matches!(
        shard.write(&mut [0u8; 6]),
        Err(FormatError::TooLarge { .. })
    ));
    assert_eq!(shard.write(&mut [0u8; 7]).unwrap(), 7);
}

#[test]
fn a_tenant_id_is_a_plain_bounded_segment() {
    assert!(check_tenant_id("acme-1.eu_west").is_ok());
    assert!(check_tenant_id(&"x".repeat(MAX_TENANT_ID_BYTES)).is_ok());
    for bad in ["", ".", "..", "a/b", "a\nb", "a\u{7}b"] {
        assert!(check_tenant_id(bad).is_err(), "{bad:?}");
    }
    assert!(matches!(
        check_tenant_id(&"x".repeat(MAX_TENANT_ID_BYTES + 1)),
        Err(FormatError::TooLarge { .. })
    ));
    let mut shard = RegistryShard::new();
    assert!(shard.insert("a\nb").is_err());
}

#[test]
fn a_tenant_lands_in_one_stable_shard() {
    assert_eq!(registry_shard_of("acme"), registry_shard_of("acme"));
    // FNV-1a 32-bit of "a" is 0xe40c292c: low byte 0x2c.
    assert_eq!(registry_shard_of("a"), 0x2c);
    let spread: std::collections::HashSet<usize> = (0..2000)
        .map(|i| registry_shard_of(&format!("tenant-{i}")))
        .collect();
    assert!(spread.len() > 200 && spread.iter().all(|s| *s < REGISTRY_SHARDS));
}

#[test]
fn keys_are_derived_from_the_base() {
    assert_eq!(registry_shard_key("", 7).unwrap(), "alerting/registry/007");
    assert_eq!(
        registry_shard_key("dep/", 255).unwrap(),
        "dep/alerting/registry/255"
    );
    assert!(registry_shard_key("dep", 1).is_err());
    assert!(registry_shard_key("", REGISTRY_SHARDS).is_err());
    assert_eq!(
        run_claim_key("dep/", "acme", "m1").unwrap(),
        "dep/alerting/runs/acme/m1"
    );
    assert!(run_claim_key("", "a/b", "m").is_err());
    assert!(run_claim_key("", "a", "m/1").is_err());
    assert!(run_claim_key("", "a", "").is_err());
}

#[test]
fn a_run_claim_is_eight_big_endian_bytes() {
    assert_eq!(
        encode_run_claim(0x0102_0304_0506_0708),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!(decode_run_claim(&[0, 0, 0, 0, 0, 0, 1, 0]).unwrap(), 256);
    assert!(decode_run_claim(&[0; 7]).is_err());
    assert!(decode_run_claim(&[0; 9]).is_err());
}

#[test]
fn the_largest_shard_is_the_entry_bound_of_the_longest_ids_and_one_byte_more_is_refused() {
    assert_eq!(MAX_REGISTRY_SHARD_BYTES, 4096 * 65);
    let mut shard = RegistryShard::new();
    for i in 0..MAX_REGISTRY_ENTRIES {
        shard.insert(&format!("{i:064}")).unwrap();
    }
    let bytes = written(&shard);
    assert_eq!(bytes.len(), MAX_REGISTRY_SHARD_BYTES);
    assert_eq!(RegistryShard::parse(&bytes).unwrap(), shard);
    let mut over = bytes;
    over.push(b'\n');
    assert!(matches!(
        RegistryShard::parse(&over),
        Err(FormatError::TooLarge { .. })
    ));
}

#[test]
fn a_namespace_keeps_its_registers_apart() {
    use oss_format::{ALERTING_NAMESPACE, registry_shard_key_in, run_claim_key_in};
    assert_eq!(ALERTING_NAMESPACE, "alerting");
    assert_eq!(
        registry_shard_key_in("dep/", "ad", 7).unwrap(),
        "dep/ad/registry/007"
    );
    assert_eq!(
        run_claim_key_in("dep/", "ad", "acme", "d1").unwrap(),
        "dep/ad/runs/acme/d1"
    );
    assert_eq!(
        registry_shard_key_in("dep/", "alerting", 7).unwrap(),
        registry_shard_key("dep/", 7).unwrap()
    );
    assert!(registry_shard_key_in("", "a/b", 0).is_err());
    assert!(registry_shard_key_in("", "", 0).is_err());
    assert!(run_claim_key_in("", "ad", "acme", "").is_err());
}
