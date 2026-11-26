//! The OpenSearch protobuf schema, loaded once from the descriptor set `build.rs` compiled.

use std::sync::LazyLock;

use prost_reflect::{DescriptorPool, MessageDescriptor};

static POOL: LazyLock<DescriptorPool> = LazyLock::new(|| {
    DescriptorPool::decode(include_bytes!(concat!(env!("OUT_DIR"), "/grpc.bin")).as_slice())
        .expect("the compiled descriptor set loads")
});

/// Builds the schema now, so the first call does not pay for it.
pub(crate) fn warm() {
    LazyLock::force(&POOL);
}

/// The message type named `org.opensearch.protobufs.<name>`.
pub fn message(name: &str) -> MessageDescriptor {
    // ALLOC: a type name, a few bytes.
    POOL.get_message_by_name(&format!("org.opensearch.protobufs.{name}"))
        .unwrap_or_else(|| panic!("the schema has no message {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_the_front_maps_exist() {
        for name in [
            "BulkRequest",
            "BulkResponse",
            "SearchRequest",
            "SearchResponse",
            "CreatePitRequest",
            "CreatePITResponse",
            "DeletePitRequest",
            "DeletePITResponse",
        ] {
            assert_eq!(message(name).name(), name);
        }
    }
}
