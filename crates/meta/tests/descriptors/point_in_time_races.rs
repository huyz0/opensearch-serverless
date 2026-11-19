//! docs/specs/point-in-time-store/SPEC.md, AC-6, AC-10 and AC-12: the
//! point-in-time store against a store that fails or lets another writer in.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use oss_blob::{BlobStore, SliceSource, SwapOutcome};
use oss_meta::{MetaError, PointInTime};

use crate::BASE;
use crate::point_in_time::{BUF, ID, buf, store_of, view};
use crate::probe::{Quirk, Quirky};

#[test]
fn a_marker_that_cannot_be_written_releases_the_view_it_began() {
    // Requests: HEAD (1), the register create (2), the marker (3) fails.
    let quirky = Arc::new(Quirky::new(Quirk::FailRequest(3)));
    let inner = Arc::clone(&quirky.inner);
    let (_, p) = store_of(quirky);
    assert!(p.create(&view(ID, 5_000), &mut buf()).is_err());
    let mut raw = buf();
    let read = inner
        .read_register(&format!("{BASE}pit-states/{ID}"), &mut raw)
        .unwrap()
        .unwrap();
    assert_eq!(
        &raw[..read.len],
        oss_meta::RELEASED,
        "the id is tombstoned, not left live"
    );
    assert!(inner.head(&format!("{BASE}pits/{ID}")).unwrap().is_none());
}

#[test]
fn an_unconditional_keep_alive_that_loses_a_race_retries_and_a_conditional_one_gives_up() {
    let racing = |expected: Option<u64>| {
        let quirky = Arc::new(Quirky::hooked(|inner: &dyn BlobStore| {
            // Another node extends to 6000 just before this one's swap.
            let key = format!("{BASE}pit-states/{ID}");
            let mut raw = vec![0u8; BUF];
            let r = inner.read_register(&key, &mut raw).unwrap().unwrap();
            let mut v = PointInTime::parse(&raw[..r.len]).unwrap();
            v.expires_at_millis = 6_000;
            let n = v.write(&mut raw).unwrap();
            let SwapOutcome::Applied(_) =
                inner.swap_register(&key, r.generation, &raw[..n]).unwrap()
            else {
                panic!("the other node's swap applies");
            };
        }));
        let (_, p) = store_of(Arc::clone(&quirky) as Arc<dyn BlobStore>);
        p.create(&view(ID, 5_000), &mut buf()).unwrap();
        // Requests to come: the state read (1), the marker read (2), then the swap (3).
        quirky.arm(3);
        let won = p.extend(ID, 8_000, expected, &mut buf()).unwrap();
        (
            won,
            p.read(ID, &mut buf()).unwrap().unwrap().expires_at_millis,
        )
    };
    assert_eq!(
        racing(None),
        (true, 8_000),
        "reads the new state and extends over it"
    );
    assert_eq!(
        racing(Some(5_000)),
        (false, 6_000),
        "the deadline the caller saw is gone"
    );
}

#[test]
fn a_keep_alive_that_loses_every_attempt_is_contended() {
    let quirky = Arc::new(Quirky::new(Quirk::ConflictSwaps(100)));
    let (_, p) = store_of(Arc::clone(&quirky) as Arc<dyn BlobStore>);
    // Registers are created by the inner store directly, since creates conflict too.
    let mut v = buf();
    let n = view(ID, 5_000).write(&mut v).unwrap();
    quirky
        .inner
        .create_register(&format!("{BASE}pit-states/{ID}"), &v[..n])
        .unwrap();
    assert!(matches!(
        p.extend(ID, 8_000, None, &mut buf()),
        Err(MetaError::Contended)
    ));
    assert!(matches!(
        p.release(ID, &mut buf()),
        Err(MetaError::Contended)
    ));
}

#[test]
fn a_collector_claiming_expiry_loses_to_a_keep_alive_that_wins_first() {
    let quirky = Arc::new(Quirky::hooked(|inner: &dyn BlobStore| {
        // Another node extends the view just before the claim's swap.
        let key = format!("{BASE}pit-states/{ID}");
        let mut raw = vec![0u8; BUF];
        let r = inner.read_register(&key, &mut raw).unwrap().unwrap();
        let mut v = PointInTime::parse(&raw[..r.len]).unwrap();
        v.expires_at_millis = 50_000;
        let n = v.write(&mut raw).unwrap();
        inner.swap_register(&key, r.generation, &raw[..n]).unwrap();
    }));
    let (now, p) = store_of(Arc::clone(&quirky) as Arc<dyn BlobStore>);
    p.create(&view(ID, 5_000), &mut buf()).unwrap();
    now.store(6_000, Ordering::SeqCst);
    // Requests to come: the listing (1), the state read (2), the claim's swap (3).
    quirky.arm(3);
    let live = p.live(&mut buf()).unwrap();
    assert_eq!(live.len(), 1, "the extended view is live, not claimed");
    assert_eq!(live[0].expires_at_millis, 50_000);
    assert!(p.read(ID, &mut buf()).unwrap().is_some());
}

#[test]
fn a_marker_already_there_is_not_removed_by_a_create_that_lost_the_id() {
    // A creator that predates state registers wrote the marker after our
    // HEAD: the inventory entry is theirs.
    let quirky = Arc::new(Quirky::hooked(|inner: &dyn BlobStore| {
        let mut raw = vec![0u8; BUF];
        let n = view(ID, 9_000).marker().write(&mut raw).unwrap();
        inner
            .put(
                &format!("{BASE}pits/{ID}"),
                &mut SliceSource::new(&raw[..n]),
                &mut [0u8; 4096],
            )
            .unwrap();
    }));
    let (_, q) = store_of(Arc::clone(&quirky) as Arc<dyn BlobStore>);
    // Requests: HEAD (1), the register create (2), the marker (3).
    quirky.arm(3);
    assert!(matches!(
        q.create(&view(ID, 5_000), &mut buf()),
        Err(MetaError::AlreadyExists)
    ));
    let marker = format!("{BASE}pits/{ID}");
    assert!(
        quirky.inner.head(&marker).unwrap().is_some(),
        "theirs stays"
    );
    let mut raw = buf();
    let state = format!("{BASE}pit-states/{ID}");
    let r = quirky
        .inner
        .read_register(&state, &mut raw)
        .unwrap()
        .unwrap();
    assert_eq!(
        &raw[..r.len],
        oss_meta::RELEASED,
        "our register is released"
    );
}

#[test]
fn a_marker_that_failed_is_kept_when_the_view_turned_out_to_be_live_under_another_writer() {
    // Requests: HEAD (1), the register create (2), the marker (3), which
    // fails after another node, in the hook, moved the register on and wrote
    // the inventory entry it owns.
    let quirky = Arc::new(Quirky::hooked_with(
        Quirk::FailRequest(3),
        |inner: &dyn BlobStore| {
            let key = format!("{BASE}pit-states/{ID}");
            let mut raw = vec![0u8; BUF];
            let r = inner.read_register(&key, &mut raw).unwrap().unwrap();
            let mut v = PointInTime::parse(&raw[..r.len]).unwrap();
            v.expires_at_millis = 9_000;
            let n = v.write(&mut raw).unwrap();
            inner.swap_register(&key, r.generation, &raw[..n]).unwrap();
            let n = v.marker().write(&mut raw).unwrap();
            inner
                .put(
                    &format!("{BASE}pits/{ID}"),
                    &mut SliceSource::new(&raw[..n]),
                    &mut [0u8; 4096],
                )
                .unwrap();
        },
    ));
    let (_, p) = store_of(Arc::clone(&quirky) as Arc<dyn BlobStore>);
    quirky.arm(3);
    assert!(p.create(&view(ID, 5_000), &mut buf()).is_err());
    let marker = format!("{BASE}pits/{ID}");
    assert!(
        quirky.inner.head(&marker).unwrap().is_some(),
        "the live view keeps its marker"
    );
    assert!(p.read(ID, &mut buf()).unwrap().is_some(), "and stays live");
}
