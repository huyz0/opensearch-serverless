//! The request MAC, checked against headers the Java shell's own
//! `TransportAuthenticator` signed (`fixtures/transport/macs.txt`), and
//! every refusal it makes.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oss_transport::auth::{
    AuthConfig, Authenticator, MAC_HEADER, SecretSource, Secrets, TOKEN_HEADER, identity,
};

use crate::{bytes, rows, unhex};

pub(crate) const K1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
pub(crate) const K0: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
pub(crate) const ROTATED: i64 = 1_700_000_000_000;
pub(crate) const NOW: i64 = 1_700_000_005_000;
pub(crate) const WINDOW: i64 = 30_000;

/// Secrets a test sets, counting refreshes; a refresh installs `next`.
#[derive(Default)]
pub(crate) struct Source {
    pub(crate) secrets: Mutex<Option<Arc<Secrets>>>,
    pub(crate) next: Mutex<Option<Arc<Secrets>>>,
    pub(crate) refreshes: AtomicUsize,
}

impl SecretSource for Source {
    fn secrets(&self) -> Option<Arc<Secrets>> {
        self.secrets.lock().unwrap().clone()
    }

    fn refresh(&self) {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        if let Some(next) = self.next.lock().unwrap().take() {
            *self.secrets.lock().unwrap() = Some(next);
        }
    }
}

pub(crate) fn secrets(generation: i64, current: &str, previous: Option<&str>) -> Arc<Secrets> {
    Arc::new(Secrets {
        generation,
        current: current.into(),
        previous: previous.map(Into::into),
        rotated_at_millis: ROTATED,
    })
}

pub(crate) fn holding(s: Option<Arc<Secrets>>) -> Arc<Source> {
    let source = Arc::new(Source::default());
    *source.secrets.lock().unwrap() = s;
    source
}

pub(crate) fn auth(
    who: &str,
    source: &Arc<Source>,
    clock: &Arc<AtomicI64>,
    capacity: usize,
) -> Authenticator {
    let clock = Arc::clone(clock);
    Authenticator::new(
        AuthConfig {
            identity: who.into(),
            window_millis: WINDOW,
            replay_capacity: capacity,
        },
        Arc::clone(source) as Arc<dyn SecretSource>,
        Arc::new(move || clock.load(Ordering::SeqCst)),
    )
}

pub(crate) fn clock(at: i64) -> Arc<AtomicI64> {
    Arc::new(AtomicI64::new(at))
}

pub(crate) fn status(result: Result<(), oss_transport::exception::RemoteException>) -> u16 {
    result.unwrap_err().http_status()
}

fn headers(field: &str) -> Vec<(String, String)> {
    if field == "-" {
        return Vec::new();
    }
    field
        .split(',')
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap();
            (unhex(k), unhex(v))
        })
        .collect()
}

#[test]
fn every_header_the_java_signed_verifies_once_for_its_recipient() {
    let mut seen = 0;
    for row in rows("macs.txt") {
        seen += 1;
        assert_eq!(row[1], K1);
        let action = unhex(&row[2]);
        let recipient = unhex(&row[3]);
        let sent = headers(&row[4]);
        let request = bytes(&row[5]);
        let header = unhex(&row[6]);
        let source = holding(Some(secrets(3, K1, Some(K0))));
        let now = clock(NOW);
        let receiver = auth(&recipient, &source, &now, 16);
        receiver
            .verify(&action, &request, Some(&header), &sent)
            .unwrap();
        // Once only.
        assert_eq!(
            status(receiver.verify(&action, &request, Some(&header), &sent)),
            403
        );
        // Not for another recipient, action, request or headers.
        let other = auth("someone/else", &source, &now, 16);
        assert_eq!(
            status(other.verify(&action, &request, Some(&header), &sent)),
            403
        );
        let fresh = auth(&recipient, &source, &now, 16);
        assert_eq!(
            status(fresh.verify("internal:other", &request, Some(&header), &sent)),
            403
        );
        let mut tampered = request.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(
            status(fresh.verify(&action, &tampered, Some(&header), &sent)),
            403
        );
        let mut more = sent.clone();
        more.push(("x-added".into(), "1".into()));
        assert_eq!(
            status(fresh.verify(&action, &request, Some(&header), &more)),
            403
        );
        // The MAC and token headers are outside the digest.
        let mut with_auth = sent.clone();
        with_auth.push((MAC_HEADER.into(), header.clone()));
        with_auth.push((TOKEN_HEADER.into(), "t".into()));
        fresh
            .verify(&action, &request, Some(&header), &with_auth)
            .unwrap();
    }
    assert_eq!(seen, 4);
}

#[test]
fn a_header_rust_signs_verifies_and_reads_as_the_javas_format() {
    let source = holding(Some(secrets(3, K1, None)));
    let now = clock(NOW);
    let sender = auth(&identity("n1", "e1"), &source, &now, 16);
    let receiver = auth(&identity("n2", "e2"), &source, &now, 16);
    let sent = vec![
        ("b".to_owned(), "2".to_owned()),
        ("a".to_owned(), "1".to_owned()),
    ];
    let header = sender.sign("act", b"body", "n2/e2", &sent).unwrap();
    let parts: Vec<&str> = header.split(':').collect();
    assert_eq!(parts.len(), 5);
    assert_eq!(parts[0], "3");
    assert_eq!(parts[1], "n1/e1");
    assert_eq!(parts[2], NOW.to_string());
    assert_eq!(parts[3].len(), 32);
    assert_eq!(parts[4].len(), 64);
    // Header order does not matter: the digest sorts them.
    let reordered = vec![sent[1].clone(), sent[0].clone()];
    receiver
        .verify("act", b"body", Some(&header), &reordered)
        .unwrap();
    // Two signatures of the same request differ by their nonce.
    assert_ne!(sender.sign("act", b"body", "n2/e2", &sent).unwrap(), header);
    // No secret yet: nothing to sign with, and nothing verifies.
    let empty = holding(None);
    let none = auth("n1/e1", &empty, &now, 16);
    assert!(none.sign("act", b"body", "n2/e2", &sent).is_none());
    assert_eq!(
        status(none.verify("act", b"body", Some(&header), &sent)),
        403
    );
    assert!(
        format!("{none:?} {:?}", secrets(3, K1, None))
            .find(K1)
            .is_none()
    );
}

#[test]
fn a_timestamp_outside_the_window_is_refused() {
    let source = holding(Some(secrets(3, K1, None)));
    let at = clock(NOW);
    let sender = auth("s/1", &source, &at, 16);
    let header = sender.sign("act", b"x", "r/1", &[]).unwrap();
    for (offset, ok) in [
        (WINDOW, true),
        (WINDOW + 1, false),
        (-WINDOW, true),
        (-WINDOW - 1, false),
    ] {
        let now = clock(NOW + offset);
        let receiver = auth("r/1", &source, &now, 16);
        assert_eq!(
            receiver.verify("act", b"x", Some(&header), &[]).is_ok(),
            ok,
            "offset {offset}"
        );
    }
    // A timestamp so far off that the age overflows.
    let far = format!("3:s/1:{}:00:00", i64::MIN);
    let receiver = auth("r/1", &source, &at, 16);
    assert_eq!(status(receiver.verify("act", b"x", Some(&far), &[])), 403);
}

#[test]
fn malformed_or_missing_headers_are_refused() {
    let source = holding(Some(secrets(3, K1, None)));
    let now = clock(NOW);
    let receiver = auth("r/1", &source, &now, 16);
    assert_eq!(status(receiver.verify("act", b"x", None, &[])), 403);
    let good = auth("s/1", &source, &now, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    let (head, mac) = good.rsplit_once(':').unwrap();
    for bad in [
        "3:s:1:00".to_owned(),
        format!("{good}:extra"),
        format!("x:s/1:{NOW}:00:00"),
        "3:s/1:soon:00:00".to_owned(),
        format!("{head}:"),
        format!("{head}:{}", &mac[1..]),
        format!("{head}:zz{}", &mac[2..]),
        // Upper case with one letter past F: the MAC is random, so a 'G' put
        // in place of each 'A' leaves one with no 'A' valid hex.
        format!("{head}:G{}", &mac.to_uppercase()[1..]),
        format!("{head}:{}", &mac[2..]),
    ] {
        assert_eq!(
            status(receiver.verify("act", b"x", Some(&bad), &[])),
            403,
            "{bad}"
        );
    }
    // Upper-case hex is hex.
    receiver
        .verify(
            "act",
            b"x",
            Some(&format!("{head}:{}", mac.to_uppercase())),
            &[],
        )
        .unwrap();
}

#[test]
fn a_full_replay_cache_refuses_until_its_nonces_expire() {
    let source = holding(Some(secrets(3, K1, None)));
    let now = clock(NOW);
    let sender = auth("s/1", &source, &now, 16);
    let receiver = auth("r/1", &source, &now, 2);
    for _ in 0..2 {
        let header = sender.sign("act", b"x", "r/1", &[]).unwrap();
        receiver.verify("act", b"x", Some(&header), &[]).unwrap();
    }
    let header = sender.sign("act", b"x", "r/1", &[]).unwrap();
    assert_eq!(
        status(receiver.verify("act", b"x", Some(&header), &[])),
        403
    );
    // One window later the remembered nonces expire; a fresh header verifies.
    now.store(NOW + WINDOW + 1, Ordering::SeqCst);
    let header = sender.sign("act", b"x", "r/1", &[]).unwrap();
    receiver.verify("act", b"x", Some(&header), &[]).unwrap();
    // A nonce still inside the window is kept: at exactly its expiry.
    let receiver = auth("r/1", &source, &now, 1);
    now.store(NOW, Ordering::SeqCst);
    let first = sender.sign("act", b"x", "r/1", &[]).unwrap();
    receiver.verify("act", b"x", Some(&first), &[]).unwrap();
    now.store(NOW + WINDOW, Ordering::SeqCst);
    let second = sender.sign("act", b"x", "r/1", &[]).unwrap();
    assert_eq!(
        status(receiver.verify("act", b"x", Some(&second), &[])),
        403
    );
}

#[test]
fn a_timestamp_whose_expiry_overflows_is_refused() {
    let source = holding(Some(secrets(3, K1, None)));
    let far = clock(i64::MAX - 1);
    let sender = auth("s/1", &source, &far, 16);
    let header = sender.sign("act", b"x", "r/1", &[]).unwrap();
    let receiver = auth("r/1", &source, &far, 16);
    assert_eq!(
        status(receiver.verify("act", b"x", Some(&header), &[])),
        403
    );
}

#[test]
fn a_guarded_handler_answers_only_signed_requests() {
    use std::time::{Duration, Instant};

    use oss_transport::auth::Guarded;
    use oss_transport::client::{Call, CallError, Client, ClientConfig};
    use oss_transport::stream::{Output, TaskId};

    use crate::support::{Echo, budget, config, echo, peer, serve};

    let source = holding(Some(secrets(3, K1, None)));
    let now = clock(NOW);
    let receiver = Arc::new(auth("node-1/eph-1", &source, &now, 16));
    let budget = budget();
    let mut handlers = oss_transport::server::Handlers::new();
    handlers.insert(
        "test:echo".into(),
        Arc::new(Guarded::new(Arc::clone(&receiver), Echo)),
    );
    let server = serve(config(), &budget, handlers);
    let client = Client::new(ClientConfig::default(), budget.clone());
    let peer = peer(&server);
    // Unsigned: refused as Java refuses, 403.
    assert!(matches!(
        echo(&client, &peer, b"x", false),
        Err(CallError::Unauthorized(_))
    ));
    // Signed over the exact request bytes and headers sent.
    let mut request = Vec::new();
    let mut out = Output::new(&mut request, 1024);
    TaskId::EMPTY.write(&mut out).unwrap();
    out.bytes(b"signed").unwrap();
    let sender = auth("s/1", &source, &now, 16);
    let sent = vec![("x-opaque-id".to_owned(), "7".to_owned())];
    let mac = sender
        .sign("test:echo", &request, "node-1/eph-1", &sent)
        .unwrap();
    let mut headers = sent.clone();
    headers.push((MAC_HEADER.to_owned(), mac));
    let call = Call {
        action: "test:echo",
        headers: &headers,
        compress: true,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let answer = client.call(&peer, call, |o| o.bytes(&request)).unwrap();
    assert_eq!(answer.message(), b"signed");
}

#[test]
fn a_mac_with_two_bytes_flipped_alike_is_refused() {
    let source = holding(Some(secrets(3, K1, None)));
    let now = clock(NOW);
    let good = auth("s/1", &source, &now, 16)
        .sign("act", b"x", "r/1", &[])
        .unwrap();
    let (head, mac) = good.rsplit_once(':').unwrap();
    let mut bytes = unhex_bytes(mac);
    bytes[0] ^= 1;
    bytes[1] ^= 1;
    let flipped: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        status(auth("r/1", &source, &now, 16).verify(
            "act",
            b"x",
            Some(&format!("{head}:{flipped}")),
            &[]
        )),
        403
    );
}

fn unhex_bytes(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn debug_shows_the_config_and_generation_but_no_key() {
    let s = secrets(3, K1, Some(K0));
    let shown = format!("{s:?}");
    assert!(shown.contains("generation: 3"), "{shown}");
    assert!(!shown.contains(K1) && !shown.contains(K0), "{shown}");
    let source = holding(Some(s));
    let shown = format!("{:?}", auth("r/1", &source, &clock(NOW), 16));
    assert!(shown.contains("r/1") && shown.contains("30000"), "{shown}");
    assert!(!shown.contains(K1), "{shown}");
}
