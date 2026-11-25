//! The node's transport server and the handler that is filled in once the
//! roles exist.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use oss_transport::exception::RemoteException;
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handler, Request};
use oss_transport::stream::Output;
use oss_transport::version::WireVersion;

impl super::Node {
    /// The identity a signed call to this node names as its recipient.
    pub fn transport_identity(&self) -> String {
        oss_transport::auth::identity(&self.node_id, &self.ephemeral)
    }

    /// The process's ephemeral id, which a peer called at the transport
    /// address must be answered under.
    pub fn ephemeral_id(&self) -> &str {
        &self.ephemeral
    }

    /// The address the node's transport server listens on.
    pub fn transport_addr(&self) -> SocketAddr {
        self.transport.as_ref().map_or_else(
            || SocketAddr::from(([0, 0, 0, 0], 0)),
            oss_transport::server::Server::address,
        )
    }
}

/// A handler that refuses until it is given the real one: the server must
/// listen (and so have an address for the lease) before the roles that
/// answer exist.
#[derive(Default)]
pub struct Late(Arc<OnceLock<Arc<dyn Handler>>>);

impl Late {
    /// A handle that sets what this handler answers with.
    pub fn slot(&self) -> Arc<OnceLock<Arc<dyn Handler>>> {
        Arc::clone(&self.0)
    }
}

impl Handler for Late {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        match self.0.get() {
            Some(handler) => handler.handle(request, out),
            None => Err(RemoteException::rejected("the node is still starting")),
        }
    }
}

/// This node as the transport handshake describes it, listening on `bound`.
pub fn describe(node_id: &str, ephemeral_id: &str, bound: SocketAddr) -> DiscoveryNode {
    let ip = bound.ip();
    DiscoveryNode {
        // ALLOC: the node's strings, once a node.
        name: node_id.to_owned(),
        id: node_id.to_owned(),
        ephemeral_id: ephemeral_id.to_owned(),
        host_name: ip.to_string(),
        host_address: ip.to_string(),
        address: TransportAddress {
            ip,
            host: ip.to_string(),
            port: i32::from(bound.port()),
        },
        stream_address: None,
        attributes: Vec::new(),
        roles: vec![NodeRole::data(), NodeRole::ingest()],
        version: WireVersion::SHELL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oss_transport::frame::VariableHeader;

    #[test]
    fn a_late_handler_refuses_until_it_is_given_the_real_one() {
        struct Ok1;
        impl Handler for Ok1 {
            fn handle(&self, _: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
                out.bytes(b"1")
                    .map_err(|_| RemoteException::rejected("full"))
            }
        }
        let late = Late::default();
        let variable = VariableHeader::default();
        let request = Request {
            action: "a",
            variable: &variable,
            message: &[],
            version: WireVersion::SHELL,
        };
        let mut sink = Vec::new();
        assert!(
            late.handle(&request, &mut Output::new(&mut sink, 16))
                .is_err()
        );
        late.slot().set(Arc::new(Ok1)).ok().expect("set once");
        late.handle(&request, &mut Output::new(&mut sink, 16))
            .expect("answers");
        assert_eq!(sink, b"1");
    }

    #[test]
    fn the_node_names_itself_and_its_address() {
        let node = describe("n1", "e1", "127.0.0.1:9301".parse().expect("addr"));
        assert_eq!((node.id.as_str(), node.ephemeral_id.as_str()), ("n1", "e1"));
        assert_eq!(node.address.port, 9301);
        assert_eq!(node.address.ip.to_string(), "127.0.0.1");
    }
}
