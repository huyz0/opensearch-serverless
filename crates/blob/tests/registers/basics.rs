//! AC-1 and AC-8 on every backend.

use oss_blob::{BlobError, SwapOutcome};

use crate::{all, is_initial, plant, read};

// AC-1
#[test]
fn a_register_is_created_swapped_and_deleted_by_generation() {
    for b in all() {
        let s = &*b.store;
        let kind = b.kind;
        assert_eq!(read(s, "r/one"), None, "{kind:?}");
        let SwapOutcome::Applied(g) = s.create_register("r/one", b"first").unwrap() else {
            panic!("{kind:?}: create refused");
        };
        assert!(is_initial(g), "{kind:?}: {g:#x}");
        assert_eq!(read(s, "r/one"), Some((g, b"first".to_vec())), "{kind:?}");
        assert!(
            matches!(
                s.create_register("r/one", b"again"),
                Ok(SwapOutcome::Conflict(_))
            ),
            "{kind:?}"
        );
        let next = g + 1;
        assert_eq!(
            s.swap_register("r/one", g, b"second"),
            Ok(SwapOutcome::Applied(next)),
            "{kind:?}"
        );
        assert_eq!(
            s.swap_register("r/one", g, b"stale"),
            Ok(SwapOutcome::Conflict(Some(next))),
            "{kind:?}"
        );
        assert_eq!(
            read(s, "r/one"),
            Some((next, b"second".to_vec())),
            "{kind:?}"
        );
        assert_eq!(s.delete_register("r/one", g), Ok(false), "{kind:?}");
        assert_eq!(
            read(s, "r/one"),
            Some((next, b"second".to_vec())),
            "{kind:?}"
        );
        assert_eq!(s.delete_register("r/one", next), Ok(true), "{kind:?}");
        assert_eq!(read(s, "r/one"), None, "{kind:?}");
        assert_eq!(s.delete_register("r/one", next), Ok(false), "{kind:?}");
        // A swap from absent creates, at a fresh random generation.
        let SwapOutcome::Applied(again) = s.swap_register("r/one", 0, b"").unwrap() else {
            panic!("{kind:?}: swap from absent refused");
        };
        assert!(is_initial(again) && again != g, "{kind:?}");
        assert_eq!(read(s, "r/one"), Some((again, Vec::new())), "{kind:?}");
        assert_eq!(
            s.swap_register("r/two", g, b"x"),
            Ok(SwapOutcome::Conflict(Some(0))),
            "{kind:?}: a swap of a missing register sees it absent"
        );
    }
}

// AC-1: what one side planted the other reads, generation first.
#[test]
fn a_register_is_its_generation_then_its_value() {
    for b in all() {
        let s = &*b.store;
        plant(s, "r/p", 0x0102_0304_0506_0708, b"planted");
        assert_eq!(
            read(s, "r/p"),
            Some((0x0102_0304_0506_0708, b"planted".to_vec())),
            "{:?}",
            b.kind
        );
        assert_eq!(
            s.swap_register("r/p", 0x0102_0304_0506_0708, b"v"),
            Ok(SwapOutcome::Applied(0x0102_0304_0506_0709)),
            "{:?}",
            b.kind
        );
        let mut whole = Vec::new();
        s.get(
            "r/p",
            oss_blob::ByteRange::Full,
            &mut [0u8; 64],
            &mut |c: &[u8]| {
                whole.extend_from_slice(c);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            whole,
            [&[1, 2, 3, 4, 5, 6, 7, 9][..], b"v"].concat(),
            "{:?}",
            b.kind
        );
    }
}

// AC-8
#[test]
fn what_does_not_fit_is_refused_never_truncated() {
    for b in all() {
        let s = &*b.store;
        let kind = b.kind;
        let total = s.meter().total();
        let huge = vec![0u8; (16 << 20) + 1];
        assert!(
            matches!(
                s.create_register("r/big", &huge),
                Err(BlobError::InvalidArgument(_))
            ),
            "{kind:?}"
        );
        assert!(
            matches!(
                s.swap_register("r/big", 0, &huge),
                Err(BlobError::InvalidArgument(_))
            ),
            "{kind:?}"
        );
        assert_eq!(
            s.meter().total(),
            total,
            "{kind:?}: refused before any request"
        );

        s.create_register("r/wide", b"0123456789").unwrap();
        let mut small = [0u8; 4];
        assert!(
            matches!(
                s.read_register("r/wide", &mut small),
                Err(BlobError::InvalidArgument(_))
            ),
            "{kind:?}"
        );
        let mut exact = [0u8; 10];
        assert_eq!(
            s.read_register("r/wide", &mut exact).unwrap().unwrap().len,
            10,
            "{kind:?}"
        );

        s.put(
            "r/short",
            &mut oss_blob::SliceSource::new(b"abc"),
            &mut [0u8; 8],
        )
        .unwrap();
        let mut buf = [0u8; 16];
        assert!(
            matches!(
                s.read_register("r/short", &mut buf),
                Err(BlobError::Protocol(_))
            ),
            "{kind:?}"
        );
        assert!(
            matches!(
                s.swap_register("r/short", 1, b"x"),
                Err(BlobError::Protocol(_))
            ),
            "{kind:?}"
        );
        assert!(
            matches!(s.delete_register("r/short", 1), Err(BlobError::Protocol(_))),
            "{kind:?}"
        );
        assert!(
            matches!(
                s.read_register("", &mut buf),
                Err(BlobError::InvalidArgument(_))
            ),
            "{kind:?}"
        );
    }
}
