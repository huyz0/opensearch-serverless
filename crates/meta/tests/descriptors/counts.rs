//! AC-7: the requests each operation sends, per backend, and never a LIST.

use oss_blob::S3Op::{Delete, Get, GetRange, Put};
use oss_format::{IndexRecord, Tombstone};
use oss_meta::DeleteMode;

use crate::{BASE, all, counted};

const NOW: i64 = 1_763_164_800_000;

#[test]
fn each_operation_sends_what_the_spec_budgets() {
    for mode in [DeleteMode::Tombstone, DeleteMode::Conditional] {
        for b in all() {
            let d = b.descriptors(mode);
            let m = b.meter();
            let mut buf = vec![0u8; 4096];
            let r = IndexRecord::new("logs", "u", 1);

            let (g, n) = counted(m, || d.create(&r, NOW, &mut buf).unwrap());
            assert_eq!(n, [(Put, 1)], "{:?} create", b.kind);
            let (_, n) = counted(m, || d.create(&r, NOW, &mut buf).unwrap_err());
            assert_eq!(n, [(Get, 1), (Put, 1)], "{:?} create, taken", b.kind);
            for (what, n) in [
                (
                    "get",
                    counted(m, || drop(d.get("logs", &mut buf).unwrap())).1,
                ),
                (
                    "resolve",
                    counted(m, || drop(d.resolve_raw("logs", &mut buf).unwrap())).1,
                ),
                (
                    "generation",
                    counted(m, || d.generation_of("logs", &mut buf).map(drop).unwrap()).1,
                ),
            ] {
                assert_eq!(n, [(Get, 1)], "{:?} {what}", b.kind);
            }
            // The generation was just read: remembered by the object stores.
            let (g, n) = counted(m, || d.update(&r, g, &mut buf).unwrap().unwrap());
            assert_eq!(
                n,
                b.budget(&[(Put, 1)], &[(GetRange, 1), (Put, 1)]),
                "{:?} update",
                b.kind
            );

            let (deleted, n) = counted(m, || d.delete_if_unchanged("logs", g, NOW).unwrap());
            assert!(deleted);
            let expected = match mode {
                DeleteMode::Conditional => b.budget(&[(Delete, 1)], &[(GetRange, 1), (Delete, 1)]),
                DeleteMode::Tombstone => b.budget(&[(Put, 2)], &[(GetRange, 1), (Put, 2)]),
            };
            assert_eq!(n, expected, "{:?} {mode:?} delete", b.kind);

            let (_, n) = counted(m, || d.delete("logs").unwrap());
            assert_eq!(n, [(Delete, 1)], "{:?} delete", b.kind);
        }
    }
}

#[test]
fn create_over_a_tombstone_reads_once_and_swaps() {
    for b in all() {
        let d = b.descriptors(DeleteMode::Tombstone);
        let mut buf = [0u8; 128];
        let n = Tombstone {
            deleted_at: 1,
            reaping_at: 0,
        }
        .write(&mut buf)
        .unwrap();
        b.store
            .create_register(&format!("{BASE}indices/logs"), &buf[..n])
            .unwrap();
        let mut buf = vec![0u8; 4096];
        let (_, n) = counted(b.meter(), || {
            d.create(&IndexRecord::new("logs", "u", 1), NOW, &mut buf)
                .unwrap()
        });
        assert_eq!(
            n,
            b.budget(&[(Get, 1), (Put, 2)], &[(Get, 1), (GetRange, 1), (Put, 2)]),
            "{:?}",
            b.kind
        );
    }
}
