//! docs/specs/reader-newest-ask, AC-5: the transport source asks a live
//! writer over a real socket, signed, and treats anything but an answer as
//! none.

use std::sync::{Arc, Mutex};

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::newest_ask::{ASK_TIMEOUT, LiveWriter, LiveWriters, TransportNewestAsk};
use oss_reader::commit_push::NewestSource;

const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

struct Secret;

impl oss_transport::auth::SecretSource for Secret {
    fn secrets(&self) -> Option<Arc<oss_transport::auth::Secrets>> {
        Some(Arc::new(oss_transport::auth::Secrets {
            generation: 1,
            current: SECRET.to_owned(),
            previous: None,
            rotated_at_millis: 0,
        }))
    }

    fn refresh(&self) {}
}

fn auth(who: &str) -> Arc<oss_transport::auth::Authenticator> {
    Arc::new(oss_transport::auth::Authenticator::new(
        oss_transport::auth::AuthConfig {
            identity: who.to_owned(),
            window_millis: 60_000,
            replay_capacity: 64,
        },
        Arc::new(Secret),
        Arc::new(|| 1_000_000),
    ))
}

/// What the writer's handler does with an ask.
#[derive(Clone, Copy)]
enum Behaviour {
    Answer,
    Silent,
    Fail,
    Garbage,
}

struct Writer {
    behaviour: Behaviour,
    asks: Arc<Mutex<Vec<oss_transport::actions::push::NewestRequest>>>,
}

impl oss_transport::server::Handler for Writer {
    fn handle(
        &self,
        request: &oss_transport::server::Request<'_>,
        out: &mut oss_transport::stream::Output<'_>,
    ) -> Result<(), oss_transport::exception::RemoteException> {
        use oss_transport::actions::push::{NewestRequest, NewestResponse};
        let ask = NewestRequest::read(&mut oss_transport::stream::Input::new(request.message))
            .expect("decodes");
        self.asks.lock().expect("lock").push(ask);
        match self.behaviour {
            Behaviour::Answer => NewestResponse {
                commit: Some(b"newest".to_vec()),
            }
            .write(out)
            .expect("writes"),
            Behaviour::Silent => std::thread::sleep(ASK_TIMEOUT.saturating_mul(3)),
            Behaviour::Fail => {
                return Err(oss_transport::exception::RemoteException::status(
                    "failed",
                    "INTERNAL_SERVER_ERROR",
                ));
            }
            Behaviour::Garbage => out.bytes(&[0xff, 0xff, 0xff]).expect("writes"),
        }
        Ok(())
    }
}

type Asks = Arc<Mutex<Vec<oss_transport::actions::push::NewestRequest>>>;

fn writer(behaviour: Behaviour) -> (oss_transport::server::Server, Asks) {
    use oss_transport::actions::push::NEWEST_ACTION;
    use oss_transport::auth::{Guarded, identity};
    use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
    use oss_transport::server::{Handlers, Server, ServerConfig};
    use oss_transport::version::WireVersion;
    let asks: Asks = Arc::default();
    let mut handlers: Handlers = std::collections::HashMap::new();
    handlers.insert(
        NEWEST_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("node-1", "eph")),
            Writer {
                behaviour,
                asks: Arc::clone(&asks),
            },
        )),
    );
    let node = DiscoveryNode {
        name: "node-1".into(),
        id: "node-1".into(),
        ephemeral_id: "eph".into(),
        host_name: "127.0.0.1".into(),
        host_address: "127.0.0.1".into(),
        address: TransportAddress {
            ip: "127.0.0.1".parse().expect("ip"),
            host: "127.0.0.1".into(),
            port: 9300,
        },
        stream_address: None,
        attributes: Vec::new(),
        roles: vec![NodeRole::data()],
        version: WireVersion::SHELL,
    };
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    let server = Server::bind(
        "127.0.0.1:0".parse().expect("addr"),
        ServerConfig::new(node, "cluster", 4),
        MemoryBudget::new(&config),
        handlers,
    )
    .expect("binds");
    (server, asks)
}

struct Live(Option<LiveWriter>);

impl LiveWriters for Live {
    fn live(&self, node_id: &str) -> Option<LiveWriter> {
        assert_eq!(node_id, "node-1");
        self.0.clone()
    }
}

fn asker(server: &oss_transport::server::Server, live: bool) -> impl NewestSource {
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    TransportNewestAsk::new(
        Arc::new(oss_transport::client::Client::new(
            oss_transport::client::ClientConfig::default(),
            MemoryBudget::new(&config),
        )),
        auth("reader-1"),
        Live(live.then(|| LiveWriter {
            address: server.address(),
            ephemeral_id: "eph".into(),
        })),
    )
}

#[test]
fn a_live_writer_is_asked_once_signed_and_its_answer_decoded() {
    let (server, asks) = writer(Behaviour::Answer);
    let source = asker(&server, true);
    assert_eq!(
        source.newest("logs", "u1", 0, "node-1"),
        Some(b"newest".to_vec())
    );
    let asks = asks.lock().expect("lock");
    assert_eq!(asks.len(), 1);
    assert_eq!(
        (
            asks[0].index_name.as_str(),
            asks[0].index_uuid.as_str(),
            asks[0].shard
        ),
        ("logs", "u1", 0)
    );
}

#[test]
fn a_writer_not_shown_live_is_not_asked() {
    let (server, asks) = writer(Behaviour::Answer);
    let source = asker(&server, false);
    assert_eq!(source.newest("logs", "u1", 0, "node-1"), None);
    assert!(asks.lock().expect("lock").is_empty());
}

#[test]
fn a_writer_that_is_silent_fails_or_answers_garbage_is_no_answer() {
    for behaviour in [Behaviour::Silent, Behaviour::Fail, Behaviour::Garbage] {
        let (server, asks) = writer(behaviour);
        let source = asker(&server, true);
        let started = std::time::Instant::now();
        assert_eq!(source.newest("logs", "u1", 0, "node-1"), None);
        assert!(
            started.elapsed() < ASK_TIMEOUT.saturating_mul(3),
            "bounded by the timeout"
        );
        assert_eq!(asks.lock().expect("lock").len(), 1);
    }
}

#[test]
fn a_signature_the_writer_does_not_accept_is_no_answer() {
    let (server, asks) = writer(Behaviour::Answer);
    let config = BudgetConfig::new(64 << 20)
        .with_pool(Pool::Transport, 64 << 20)
        .expect("fits");
    let wrong = TransportNewestAsk::new(
        Arc::new(oss_transport::client::Client::new(
            oss_transport::client::ClientConfig::default(),
            MemoryBudget::new(&config),
        )),
        auth("reader-1"),
        Live(Some(LiveWriter {
            address: server.address(),
            // Not the process the writer is.
            ephemeral_id: "another".into(),
        })),
    );
    assert_eq!(wrong.newest("logs", "u1", 0, "node-1"), None);
    assert!(asks.lock().expect("lock").is_empty());
}
