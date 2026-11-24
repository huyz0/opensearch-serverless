//! The connection linger: what it discards of a refused body. Under #[cfg(test)].

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;

use super::{LINGER_LIMIT, linger};

fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let client = TcpStream::connect(listener.local_addr().expect("addr")).expect("connect");
    let (server, _) = listener.accept().expect("accept");
    (server, client)
}

#[test]
fn linger_discards_at_most_its_limit() {
    let (server, mut client) = pair();
    let writer = thread::spawn(move || {
        let chunk = [7u8; 4096];
        // Four times the limit; the server stops reading at the limit.
        for _ in 0..(LINGER_LIMIT * 4 / 4096) {
            if client.write_all(&chunk).is_err() {
                break;
            }
        }
    });
    assert_eq!(linger(&server), LINGER_LIMIT);
    drop(server);
    let _ = writer.join();
}

#[test]
fn linger_reads_what_is_in_flight_then_stops_when_the_client_is_quiet() {
    let (server, mut client) = pair();
    client.write_all(&[1u8; 10_000]).expect("send");
    let started = std::time::Instant::now();
    assert_eq!(linger(&server), 10_000);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "waited past the idle limit"
    );
    drop(client);
}
