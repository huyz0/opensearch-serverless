//! `DiscoveryNode` and `TransportAddress` as the transport handshake writes
//! them: how a node describes itself to a peer.
//!
//! Only the wire versions this node accepts are written, so the version
//! gates the Java passes (a stream address from 3.2.0, attributes left out
//! of `writeTo` from 2.17.0) are always passed.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::error::WireError;
use crate::stream::{Input, Output};
use crate::version::WireVersion;

/// A socket address as `TransportAddress` writes it: the IP's bytes, the
/// host string, the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportAddress {
    /// The IP address.
    pub ip: IpAddr,
    /// The host string (`InetSocketAddress.getHostString`): the name it was
    /// created with, or the IP's text.
    pub host: String,
    /// The port, an int on the wire.
    pub port: i32,
}

impl TransportAddress {
    /// `TransportAddress.writeTo`.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        match self.ip {
            IpAddr::V4(v4) => {
                out.byte(4)?;
                out.bytes(&v4.octets())?;
            }
            IpAddr::V6(v6) => {
                out.byte(16)?;
                out.bytes(&v6.octets())?;
            }
        }
        out.string(&self.host)?;
        out.int(self.port)
    }

    /// `new TransportAddress(StreamInput)`.
    ///
    /// # Errors
    ///
    /// [`WireError::Malformed`] for an address of neither 4 nor 16 bytes;
    /// as [`Input::string`] otherwise.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a transport address";
        let ip = match input.byte(WHAT)? {
            4 => {
                let mut octets = [0u8; 4];
                octets.copy_from_slice(input.take(4, WHAT)?);
                IpAddr::V4(Ipv4Addr::from(octets))
            }
            16 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(input.take(16, WHAT)?);
                IpAddr::V6(Ipv6Addr::from(octets))
            }
            _ => return Err(WireError::malformed(WHAT, "an address is 4 or 16 bytes")),
        };
        let host = input.string(WHAT)?;
        let port = input.int(WHAT)?;
        Ok(Self { ip, host, port })
    }
}

/// A node role as the wire names it (`DiscoveryNodeRole`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeRole {
    /// The role's name, `data` or `ingest` for the shell's nodes.
    pub name: String,
    /// Its abbreviation, `d` or `i`.
    pub abbreviation: String,
    /// Whether a node with it can hold data.
    pub can_contain_data: bool,
}

impl NodeRole {
    /// The built-in data role.
    pub fn data() -> Self {
        Self::new("data", "d", true)
    }

    /// The built-in ingest role.
    pub fn ingest() -> Self {
        Self::new("ingest", "i", false)
    }

    /// A role from its three fields.
    pub fn new(name: &str, abbreviation: &str, can_contain_data: bool) -> Self {
        Self {
            // ALLOC: one name and abbreviation a role, a handful a node.
            name: name.to_owned(),
            // ALLOC: as above.
            abbreviation: abbreviation.to_owned(),
            can_contain_data,
        }
    }
}

/// A node as `DiscoveryNode` writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryNode {
    /// `node.name`.
    pub name: String,
    /// The node id (the lease's node id).
    pub id: String,
    /// The process's ephemeral id (the lease's); two nodes are equal when
    /// these are.
    pub ephemeral_id: String,
    /// The host name.
    pub host_name: String,
    /// The host address text.
    pub host_address: String,
    /// The transport address.
    pub address: TransportAddress,
    /// The streaming transport address, when the node has one.
    pub stream_address: Option<TransportAddress>,
    /// Attributes, in the order written (the Java writes a hash map's
    /// order, which a reader must not depend on).
    pub attributes: Vec<(String, String)>,
    /// Roles, sorted by name as the Java's sorted set writes them.
    pub roles: Vec<NodeRole>,
    /// The node's version.
    pub version: WireVersion,
}

impl DiscoveryNode {
    /// `writeToWithAttribute` when `with_attributes`, else `writeTo` at an
    /// accepted version (attributes written as none). The transport
    /// handshake answers with attributes.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>, with_attributes: bool) -> Result<(), WireError> {
        out.string(&self.name)?;
        out.string(&self.id)?;
        out.string(&self.ephemeral_id)?;
        out.string(&self.host_name)?;
        out.string(&self.host_address)?;
        self.address.write(out)?;
        match &self.stream_address {
            None => out.bool(false)?,
            Some(address) => {
                out.bool(true)?;
                address.write(out)?;
            }
        }
        if with_attributes {
            out.string_map(&self.attributes)?;
        } else {
            out.count(0)?;
        }
        out.count(self.roles.len())?;
        for role in &self.roles {
            out.string(&role.name)?;
            out.string(&role.abbreviation)?;
            out.bool(role.can_contain_data)?;
        }
        self.version.write(out)
    }

    /// `new DiscoveryNode(StreamInput)`; roles come back sorted by name.
    ///
    /// # Errors
    ///
    /// As the stream reads.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a discovery node";
        let name = input.string(WHAT)?;
        let id = input.string(WHAT)?;
        let ephemeral_id = input.string(WHAT)?;
        let host_name = input.string(WHAT)?;
        let host_address = input.string(WHAT)?;
        let address = TransportAddress::read(input)?;
        let stream_address = if input.bool(WHAT)? {
            Some(TransportAddress::read(input)?)
        } else {
            None
        };
        let attributes = input.string_map(WHAT)?;
        // A role is at least two empty strings and a boolean.
        let count = input.count(3, WHAT)?;
        // ALLOC: at most one role per three remaining input bytes (checked by `count`).
        let mut roles = Vec::with_capacity(count);
        for _ in 0..count {
            let name = input.string(WHAT)?;
            let abbreviation = input.string(WHAT)?;
            let can_contain_data = input.bool(WHAT)?;
            roles.push(NodeRole {
                name,
                abbreviation,
                can_contain_data,
            });
        }
        // The Java collects roles into a sorted set by name: sorted, one per name.
        roles.sort();
        roles.dedup_by(|a, b| a.name == b.name);
        let version = WireVersion::read(input)?;
        Ok(Self {
            name,
            id,
            ephemeral_id,
            host_name,
            host_address,
            address,
            stream_address,
            attributes,
            roles,
            version,
        })
    }
}
