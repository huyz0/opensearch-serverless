//! The server and client over loopback: both handshakes, calls, failures
//! told apart, and the bounds that keep a channel open or close it.

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use oss_mem::Pool;
use oss_transport::client::{Call, CallError, Client, ClientConfig, PeerId};
use oss_transport::exception::{ExceptionKind, ids};

use crate::support::*;

#[test]
fn a_call_is_answered_plain_and_compressed_and_reuses_its_socket() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(ClientConfig::default(), budget.clone());
    let peer = peer(&server);
    assert_eq!(echo(&client, &peer, b"hello", false).unwrap(), b"hello");
    let big = vec![7u8; 200_000];
    assert_eq!(echo(&client, &peer, &big, true).unwrap(), big);
    assert_eq!(echo(&client, &peer, b"", false).unwrap(), b"");
    assert_eq!(client.sockets(), 1, "an idle socket is reused");
    assert_eq!(server.channels(), 1);
    drop(client);
    assert!(
        eventually(|| server.channels() == 0),
        "a closed client ends its channel"
    );
    assert!(
        eventually(|| budget.in_use(Pool::Transport) == 0),
        "every frame is given back"
    );
}

#[test]
fn concurrent_calls_open_at_most_the_sockets_per_peer() {
    let budget = budget();
    let server = serve(
        config(),
        &budget,
        handlers(vec![(
            "test:slow",
            Arc::new(Slow(Duration::from_millis(100))),
        )]),
    );
    let client = Arc::new(Client::new(
        ClientConfig {
            connections_per_peer: 2,
            ..ClientConfig::default()
        },
        budget.clone(),
    ));
    let peer = peer(&server);
    let spawn = || {
        let client = Arc::clone(&client);
        let peer = peer.clone();
        thread::spawn(move || {
            client
                .call(&peer, call("test:slow", false, 5_000), |_| Ok(()))
                .map(|_| ())
        })
    };
    // One call in flight on the first socket first, so the next sees it busy.
    let mut calls = vec![spawn()];
    assert!(eventually(|| client.sockets() == 1));
    thread::sleep(Duration::from_millis(20));
    calls.extend((0..5).map(|_| spawn()));
    for c in calls {
        c.join().unwrap().unwrap();
    }
    // Busy sockets do not take a new call while the peer has room for another.
    assert_eq!(client.sockets(), 2);
}

#[test]
fn an_unknown_action_answers_action_not_found_wrapped_as_the_node_writes_it() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(ClientConfig::default(), budget.clone());
    let failure = remote(
        client
            .call(&peer(&server), call("test:missing", false, 5_000), |_| {
                Ok(())
            })
            .map(|a| a.message().to_vec()),
    );
    assert_eq!(
        failure.kind,
        Some(ExceptionKind::OpenSearch(ids::REMOTE_TRANSPORT))
    );
    assert_eq!(
        failure.message.as_deref(),
        Some("[w1][127.0.0.1:9300][test:missing]")
    );
    let root = failure.root();
    assert_eq!(
        root.kind,
        Some(ExceptionKind::OpenSearch(ids::ACTION_NOT_FOUND))
    );
    assert_eq!(
        root.message.as_deref(),
        Some("No handler for action [test:missing]")
    );
    assert_eq!(failure.http_status(), 500);
    // The channel stays usable.
    assert_eq!(echo(&client, &peer(&server), b"x", false).unwrap(), b"x");
}

#[test]
fn a_refused_authentication_is_unauthorized_and_another_failure_is_remote() {
    let budget = budget();
    let server = serve(
        config(),
        &budget,
        handlers(vec![
            ("test:forbid", Arc::new(Forbid)),
            ("test:unavailable", Arc::new(Unavailable)),
        ]),
    );
    let client = Client::new(ClientConfig::default(), budget.clone());
    match client.call(
        &peer(&server),
        call("test:forbid", false, 5_000),
        |_| Ok(()),
    ) {
        Err(CallError::Unauthorized(e)) => assert_eq!(e.http_status(), 403),
        other => panic!("{other:?}"),
    }
    match client.call(
        &peer(&server),
        call("test:unavailable", false, 5_000),
        |_| Ok(()),
    ) {
        Err(CallError::Remote(e)) => assert_eq!(e.http_status(), 503),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_full_worker_queue_answers_a_rejection() {
    let budget = budget();
    let barrier = Arc::new(Barrier::new(2));
    let started = Arc::new(AtomicUsize::new(0));
    let mut config = config();
    config.workers = 1;
    config.queue = 1;
    let server = serve(
        config,
        &budget,
        handlers(vec![(
            "test:hold",
            Arc::new(Hold(Arc::clone(&barrier), Arc::clone(&started))),
        )]),
    );
    let client = Arc::new(Client::new(ClientConfig::default(), budget.clone()));
    let peer = peer(&server);
    let spawn = |client: &Arc<Client>| {
        let client = Arc::clone(client);
        let peer = peer.clone();
        thread::spawn(move || {
            client
                .call(&peer, call("test:hold", false, 10_000), |_| Ok(()))
                .map(|_| ())
        })
    };
    // One running, one queued, the third refused.
    let running = spawn(&client);
    assert!(eventually(|| started.load(Ordering::SeqCst) == 1));
    let queued = spawn(&client);
    thread::sleep(Duration::from_millis(100));
    match client.call(&peer, call("test:hold", false, 5_000), |_| Ok(())) {
        Err(CallError::Remote(e)) => {
            assert_eq!(e.root().kind, Some(ExceptionKind::Jdk(18)));
            assert_eq!(e.http_status(), 429);
        }
        other => panic!("{other:?}"),
    }
    barrier.wait();
    running.join().unwrap().unwrap();
    barrier.wait();
    queued.join().unwrap().unwrap();
}

#[test]
fn a_frame_past_the_servers_bound_is_answered_and_the_channel_stays_open() {
    let budget = budget();
    let mut config = config();
    config.max_frame_bytes = 4096;
    let server = serve(config, &budget, handlers(vec![]));
    let client = Client::new(ClientConfig::default(), budget.clone());
    let peer = peer(&server);
    let failure = remote(echo(&client, &peer, &[1u8; 8192], false));
    assert_eq!(failure.http_status(), 413);
    assert_eq!(echo(&client, &peer, b"after", false).unwrap(), b"after");
    // A compressed message that inflates past the bound is refused too.
    let failure = remote(echo(&client, &peer, &[0u8; 64 * 1024], true));
    assert_eq!(failure.http_status(), 413);
    assert_eq!(echo(&client, &peer, b"again", false).unwrap(), b"again");
    assert_eq!(client.sockets(), 1);
}

#[test]
fn a_frame_past_the_clients_bound_is_too_large_before_sending() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(
        ClientConfig {
            max_frame_bytes: 1024,
            ..ClientConfig::default()
        },
        budget.clone(),
    );
    assert!(matches!(
        echo(&client, &peer(&server), &[0u8; 2048], false),
        Err(CallError::TooLarge)
    ));
}

#[test]
fn an_answer_past_the_clients_bound_fails_the_call_alone() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(
        ClientConfig {
            max_frame_bytes: 4096,
            ..ClientConfig::default()
        },
        budget.clone(),
    );
    let peer = peer(&server);
    // A compressed request is small; its compressed answer inflates past 4 KiB.
    assert!(matches!(
        echo(&client, &peer, &[0u8; 8192], true),
        Err(CallError::TooLarge)
    ));
    assert_eq!(echo(&client, &peer, b"ok", false).unwrap(), b"ok");
}

#[test]
fn a_closed_port_is_refused() {
    let address = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let client = Client::new(ClientConfig::default(), budget());
    let peer = PeerId {
        address,
        ephemeral_id: None,
    };
    assert!(matches!(
        echo(&client, &peer, b"x", false),
        Err(CallError::Refused)
    ));
    assert_eq!(client.sockets(), 0);
}

#[test]
fn another_process_at_the_address_is_the_wrong_peer() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(ClientConfig::default(), budget.clone());
    let peer = PeerId {
        address: server.address(),
        ephemeral_id: Some("eph-old".into()),
    };
    assert!(matches!(
        echo(&client, &peer, b"x", false),
        Err(CallError::WrongPeer(_))
    ));
    assert_eq!(client.sockets(), 0);
    let any = PeerId {
        address: server.address(),
        ephemeral_id: None,
    };
    assert_eq!(echo(&client, &any, b"x", false).unwrap(), b"x");
}

#[test]
fn a_slow_answer_times_out_and_the_socket_serves_the_next_call() {
    let budget = budget();
    let server = serve(
        config(),
        &budget,
        handlers(vec![(
            "test:slow",
            Arc::new(Slow(Duration::from_millis(300))),
        )]),
    );
    let client = Client::new(ClientConfig::default(), budget.clone());
    let peer = peer(&server);
    assert!(matches!(
        client.call(&peer, call("test:slow", false, 100), |_| Ok(())),
        Err(CallError::Timeout)
    ));
    assert_eq!(echo(&client, &peer, b"x", false).unwrap(), b"x");
    // A deadline already passed fails before sending.
    assert!(matches!(
        client.call(&peer, call("test:echo", false, 0), |_| Ok(())),
        Err(CallError::Timeout)
    ));
}

#[test]
fn a_stopped_server_fails_the_open_socket_and_then_refuses() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(ClientConfig::default(), budget.clone());
    let peer = peer(&server);
    assert_eq!(echo(&client, &peer, b"x", false).unwrap(), b"x");
    drop(server);
    assert!(
        eventually(|| client.sockets() == 0),
        "the stop closed the channel"
    );
    assert!(matches!(
        echo(&client, &peer, b"x", false),
        Err(CallError::Refused)
    ));
}

#[test]
fn a_pooled_socket_to_another_process_is_closed_not_reused() {
    let budget = budget();
    let server = serve(config(), &budget, handlers(vec![]));
    let client = Client::new(ClientConfig::default(), budget.clone());
    assert_eq!(echo(&client, &peer(&server), b"x", false).unwrap(), b"x");
    assert_eq!(client.sockets(), 1);
    // The lease now names a new process at the same address.
    let moved = PeerId {
        address: server.address(),
        ephemeral_id: Some("eph-2".into()),
    };
    assert!(matches!(
        echo(&client, &moved, b"x", false),
        Err(CallError::WrongPeer(_))
    ));
    assert!(eventually(|| client.sockets() == 0));
}

#[test]
fn a_calls_headers_reach_the_handler() {
    let budget = budget();
    let server = serve(
        config(),
        &budget,
        handlers(vec![("test:header", Arc::new(HeaderEcho))]),
    );
    let client = Client::new(ClientConfig::default(), budget.clone());
    let headers = [("x-test".to_owned(), "carried".to_owned())];
    let call = Call {
        action: "test:header",
        headers: &headers,
        compress: false,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    let answer = client.call(&peer(&server), call, |_| Ok(())).unwrap();
    let mut input = oss_transport::stream::Input::new(answer.message());
    assert_eq!(input.string("a header").unwrap(), "carried");
}

#[test]
fn only_a_security_failure_with_401_or_403_is_unauthorized() {
    let budget = budget();
    let server = serve(
        config(),
        &budget,
        handlers(vec![
            ("test:status-403", Arc::new(StatusForbidden)),
            ("test:security-503", Arc::new(SecurityUnavailable)),
        ]),
    );
    let client = Client::new(ClientConfig::default(), budget.clone());
    for action in ["test:status-403", "test:security-503"] {
        match client.call(&peer(&server), call(action, false, 5_000), |_| Ok(())) {
            Err(CallError::Remote(_)) => {}
            other => panic!("{action}: {other:?}"),
        }
    }
}
