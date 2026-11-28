//! docs/specs/alerting/SPEC.md, slice 1: the registry's shards and run
//! claims over the three backends, with the protocol's interleavings.

use oss_blob::S3Op::{Get, GetRange, Put};
use oss_format::{MAX_REGISTRY_ENTRIES, REGISTRY_SHARDS, registry_shard_of};
use oss_meta::{AlertRegistry, MetaError};
use oss_testkit::budget;

use crate::{BASE, Backend, Kind, all, counted};

fn total(sent: &[(oss_blob::S3Op, u64)]) -> u64 {
    sent.iter().map(|(_, n)| n).sum()
}

fn registry(b: &Backend) -> AlertRegistry {
    AlertRegistry::new(std::sync::Arc::clone(&b.store), BASE).unwrap()
}

/// Tenants of one shard, for a test that needs two in the same register.
fn same_shard(first: &str, count: usize) -> Vec<String> {
    let shard = registry_shard_of(first);
    (0..)
        .map(|i| format!("peer-{i}"))
        .filter(|t| registry_shard_of(t) == shard)
        .take(count)
        .collect()
}

#[test]
fn a_tenant_is_registered_read_and_removed_in_its_shard() {
    for b in all() {
        let r = registry(&b);
        assert!(!r.is_registered("acme").unwrap());
        r.register("acme").unwrap();
        assert!(r.is_registered("acme").unwrap());
        let shard = registry_shard_of("acme");
        assert_eq!(r.tenants(shard).unwrap(), ["acme"]);
        assert!(r.unregister_unless("acme", &mut || Ok(false)).unwrap());
        assert!(!r.is_registered("acme").unwrap());
        assert!(!r.unregister_unless("acme", &mut || Ok(false)).unwrap());
    }
}

#[test]
fn registering_costs_one_read_and_one_write_even_when_the_entry_is_there() {
    for b in all() {
        let r = registry(&b);
        let (_, first) = counted(b.meter(), || r.register("acme").unwrap());
        assert_eq!(first, [(Get, 1), (Put, 1)], "{:?}", b.kind);
        let (_, again) = counted(b.meter(), || r.register("acme").unwrap());
        let swap = b.budget(&[(Get, 1), (Put, 1)], &[(Get, 1), (GetRange, 1), (Put, 1)]);
        assert_eq!(again, swap, "{:?}", b.kind);
        if b.kind == Kind::Emulator {
            budget::check(
                "alerting.register.requests",
                total(&again),
                "a registration",
            );
        }
        assert_eq!(r.tenants(registry_shard_of("acme")).unwrap(), ["acme"]);
    }
}

#[test]
fn a_removal_that_finds_an_enabled_monitor_keeps_the_entry_and_writes_nothing() {
    for b in all() {
        let r = registry(&b);
        r.register("acme").unwrap();
        let (removed, sent) = counted(b.meter(), || {
            r.unregister_unless("acme", &mut || Ok(true)).unwrap()
        });
        assert!(!removed);
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
        assert!(r.is_registered("acme").unwrap());
    }
}

#[test]
fn a_removal_costs_one_read_and_one_write() {
    for b in all() {
        let r = registry(&b);
        r.register("acme").unwrap();
        let (removed, sent) = counted(b.meter(), || {
            r.unregister_unless("acme", &mut || Ok(false)).unwrap()
        });
        assert!(removed);
        let swap = b.budget(&[(Get, 1), (Put, 1)], &[(Get, 1), (GetRange, 1), (Put, 1)]);
        assert_eq!(sent, swap, "{:?}", b.kind);
        if b.kind == Kind::Emulator {
            budget::check("alerting.unregister.requests", total(&sent), "a removal");
        }
    }
}

/// B's monitor write and shard swap land between A's shard read and A's
/// swap: A's swap loses, A asks again, and the answer is now yes.
#[test]
fn a_registration_between_the_removals_read_and_swap_keeps_the_entry() {
    for b in all() {
        let r = registry(&b);
        r.register("acme").unwrap();
        let mut asked = 0;
        let removed = r
            .unregister_unless("acme", &mut || {
                asked += 1;
                if asked == 1 {
                    // The adder's monitor write has landed, and its swap.
                    r.register("acme").unwrap();
                    Ok(false)
                } else {
                    Ok(true)
                }
            })
            .unwrap();
        assert!(!removed, "{:?}", b.kind);
        assert_eq!(asked, 2, "{:?}", b.kind);
        assert!(r.is_registered("acme").unwrap());
    }
}

/// The removal wins first: the adder, whose monitor write came after the
/// removal's question, registers after and the entry is back.
#[test]
fn a_registration_after_the_removal_adds_the_entry_back() {
    for b in all() {
        let r = registry(&b);
        r.register("acme").unwrap();
        assert!(r.unregister_unless("acme", &mut || Ok(false)).unwrap());
        assert!(!r.is_registered("acme").unwrap());
        r.register("acme").unwrap();
        assert!(r.is_registered("acme").unwrap());
    }
}

/// A removal that dies after its read leaves the entry, and one that dies
/// after the question does too: nothing is written before the swap.
#[test]
fn a_removal_that_stops_before_its_swap_leaves_the_entry() {
    for b in all() {
        let r = registry(&b);
        r.register("acme").unwrap();
        let mut asked = 0;
        let out = r.unregister_unless("acme", &mut || {
            asked += 1;
            Err(MetaError::InvalidArgument("stopped"))
        });
        assert!(out.is_err());
        assert_eq!(asked, 1);
        assert!(r.is_registered("acme").unwrap());
    }
}

#[test]
fn a_removal_that_keeps_losing_its_swap_gives_up_to_contention() {
    for b in all() {
        let r = registry(&b);
        r.register("acme").unwrap();
        let out = r.unregister_unless("acme", &mut || {
            r.register("acme").unwrap();
            Ok(false)
        });
        assert!(matches!(out, Err(MetaError::Contended)), "{:?}", b.kind);
        assert!(r.is_registered("acme").unwrap());
    }
}

#[test]
fn two_tenants_of_one_shard_share_its_register() {
    for b in all() {
        let r = registry(&b);
        let tenants = same_shard("acme", 2);
        for t in &tenants {
            r.register(t).unwrap();
        }
        let shard = registry_shard_of(&tenants[0]);
        assert_eq!(r.tenants(shard).unwrap().len(), 2);
        assert!(r.unregister_unless(&tenants[0], &mut || Ok(false)).unwrap());
        assert_eq!(r.tenants(shard).unwrap(), [tenants[1].clone()]);
    }
}

#[test]
fn a_full_shard_refuses_a_new_tenant_and_keeps_a_present_one() {
    let b = &all()[0];
    let r = registry(b);
    for t in same_shard("acme", MAX_REGISTRY_ENTRIES) {
        r.register(&t).unwrap();
    }
    let extra = same_shard("acme", MAX_REGISTRY_ENTRIES + 1).pop().unwrap();
    let out = r.register(&extra);
    assert!(matches!(out, Err(MetaError::Format(_))), "{out:?}");
    assert!(!r.is_registered(&extra).unwrap());
    let present = same_shard("acme", 1).pop().unwrap();
    r.register(&present).unwrap();
}

#[test]
fn an_invalid_tenant_id_sends_nothing() {
    for b in all() {
        let r = registry(&b);
        let (_, sent) = counted(b.meter(), || {
            assert!(r.register("a/b").is_err());
            assert!(r.is_registered("").is_err());
            assert!(r.unregister_unless("a\nb", &mut || Ok(false)).is_err());
            assert!(r.claim("a/b", "m", 1, None).is_err());
        });
        assert!(sent.is_empty(), "{:?}", b.kind);
    }
}

#[test]
fn every_shard_reads_empty_before_anything_is_registered() {
    let b = &all()[0];
    let r = registry(b);
    assert!((0..REGISTRY_SHARDS).all(|s| r.tenants(s).unwrap().is_empty()));
}

#[test]
fn a_slot_is_claimed_once_and_only_upwards() {
    for b in all() {
        let r = registry(&b);
        let (first, sent) = counted(b.meter(), || r.claim("acme", "m1", 10, None).unwrap());
        assert!(first.won);
        assert_eq!(sent, [(Get, 1), (Put, 1)], "{:?}", b.kind);
        if b.kind == Kind::Emulator {
            budget::check("alerting.claim.requests", total(&sent), "a first claim");
        }
        let (again, sent) = counted(b.meter(), || r.claim("acme", "m1", 10, None).unwrap());
        assert!(!again.won);
        assert_eq!(sent, [(Get, 1)], "{:?}", b.kind);
        assert!(!r.claim("acme", "m1", 9, None).unwrap().won);
        assert!(r.claim("acme", "m1", 11, None).unwrap().won);
        assert!(
            r.claim("acme", "m2", 1, None).unwrap().won,
            "another monitor's own claim"
        );
    }
}

#[test]
fn a_hinted_claim_is_one_request_and_a_stale_hint_falls_back_to_a_read() {
    for b in all() {
        let r = registry(&b);
        let first = r.claim("acme", "m1", 1, None).unwrap();
        let (second, sent) = counted(b.meter(), || {
            r.claim("acme", "m1", 2, Some(first.generation)).unwrap()
        });
        assert!(second.won);
        if b.kind != Kind::Fs {
            assert_eq!(sent, [(Put, 1)], "{:?}", b.kind);
            budget::check(
                "alerting.claim_hinted.requests",
                total(&sent),
                "a hinted claim",
            );
        }
        // Another node claimed slot 3 meanwhile: the hint is stale, the swap
        // loses, and the read finds the slot taken.
        assert!(r.claim("acme", "m1", 3, None).unwrap().won);
        let lost = r.claim("acme", "m1", 3, Some(second.generation)).unwrap();
        assert!(!lost.won);
    }
}

/// The register moves between a claimer's read and its swap: the swap loses,
/// the claimer reads again and finds the slot taken.
#[test]
fn of_two_claimers_of_one_slot_exactly_one_wins() {
    for b in all() {
        let r = registry(&b);
        let mut moved = false;
        let loser = r
            .claim_between("acme", "m1", 5, None, &mut || {
                if !moved {
                    moved = true;
                    assert!(r.claim("acme", "m1", 5, None).unwrap().won);
                }
            })
            .unwrap();
        assert!(!loser.won, "{:?}", b.kind);
    }
}

#[test]
fn a_claim_that_keeps_losing_its_swap_gives_up_to_contention() {
    for b in all() {
        let r = registry(&b);
        let mut n = 0;
        let out = r.claim_between("acme", "m1", 50, None, &mut || {
            // Another node raises the register, below 50, after every read.
            n += 1;
            assert!(r.claim("acme", "m1", n, None).unwrap().won);
        });
        assert!(matches!(out, Err(MetaError::Contended)), "{:?}", b.kind);
        assert_eq!(n, 8, "{:?}", b.kind);
    }
}

#[test]
fn a_damaged_claim_register_is_refused() {
    let b = &all()[0];
    let r = registry(b);
    let key = oss_format::run_claim_key(BASE, "acme", "m1").unwrap();
    b.store.create_register(&key, &[1, 2, 3]).unwrap();
    assert!(matches!(
        r.claim("acme", "m1", 1, None),
        Err(MetaError::Format(_))
    ));
}

#[test]
fn two_namespaces_share_a_store_without_seeing_each_other() {
    use oss_meta::ScheduleRegistry;
    for b in all() {
        let alerting = registry(&b);
        let ad =
            ScheduleRegistry::in_namespace(std::sync::Arc::clone(&b.store), BASE, "ad").unwrap();
        alerting.register("acme").unwrap();
        assert!(!ad.is_registered("acme").unwrap());
        ad.register("other").unwrap();
        assert!(!alerting.is_registered("other").unwrap());
        assert!(ad.claim("acme", "d1", 5, None).unwrap().won);
        assert!(alerting.claim("acme", "d1", 5, None).unwrap().won);
        assert!(!ad.claim("acme", "d1", 5, None).unwrap().won);
        assert!(
            ScheduleRegistry::in_namespace(std::sync::Arc::clone(&b.store), BASE, "a/b").is_err()
        );
    }
}
