//! The frame reader's memory paths: a frame is dropped and answered, not
//! read, when the transport pool cannot hold it.

use std::io::Cursor;

use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_transport::channel::{ChannelError, FrameReader, Inbound};
use oss_transport::frame::{self, FrameMeta, Status, VariableHeader};
use oss_transport::version::WireVersion;

use crate::support::MIB;

fn pool(bytes: usize) -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(bytes)
            .with_pool(Pool::Transport, bytes)
            .unwrap(),
    )
}

fn frame(compress: bool, action: &str, message: &[u8]) -> Vec<u8> {
    let variable = VariableHeader {
        action: action.into(),
        ..VariableHeader::default()
    };
    let meta = FrameMeta {
        request_id: 8,
        status: Status::request(compress, false),
        version: WireVersion::SHELL,
    };
    let mut buf = Vec::new();
    frame::encode(&mut buf, MIB, meta, &variable, |o| o.bytes(message)).unwrap();
    buf
}

fn read(budget: &MemoryBudget, bytes: Vec<u8>) -> Result<Option<Inbound>, ChannelError> {
    FrameReader::new(budget.clone(), MIB).read(&mut Cursor::new(bytes), |_, _| Ok(()))
}

fn rejected(result: Result<Option<Inbound>, ChannelError>) -> u16 {
    match result {
        Ok(Some(Inbound::Dropped {
            request_id: 8,
            is_request: true,
            error,
        })) => error.http_status(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_frame_the_pool_cannot_hold_is_dropped_as_a_rejection() {
    // The variable header alone is more than the pool.
    let tiny = pool(8);
    assert_eq!(
        rejected(read(&tiny, frame(false, "test:a-long-action-name", b""))),
        429
    );
    // The header fits, the message does not.
    let small = pool(256);
    assert_eq!(rejected(read(&small, frame(false, "t", &[1u8; 1024]))), 429);
    // A compressed message that inflates past what the pool can spare.
    assert_eq!(
        rejected(read(&small, frame(true, "t", &[0u8; 64 * 1024]))),
        429
    );
    assert_eq!(small.in_use(Pool::Transport), 0);
    // With room, the same compressed frame is read whole.
    let roomy = pool(4 * MIB);
    match read(&roomy, frame(true, "t", &[0u8; 64 * 1024])) {
        Ok(Some(Inbound::Frame(f))) => assert_eq!(f.message.len(), 64 * 1024),
        other => panic!("{other:?}"),
    }
    assert_eq!(roomy.in_use(Pool::Transport), 0);
}

#[test]
fn a_cut_prefix_or_a_broken_deflate_stream_is_an_error() {
    let budget = pool(MIB);
    assert!(matches!(
        read(&budget, b"ES\0".to_vec()),
        Err(ChannelError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof
    ));
    assert!(matches!(read(&budget, Vec::new()), Ok(None)));
    let mut broken = frame(false, "t", b"DFL\0\xff\xff\xff");
    // Set the compressed bit after the request id.
    broken[14] |= 0x04;
    assert!(matches!(read(&budget, broken), Err(ChannelError::Wire(_))));
}
