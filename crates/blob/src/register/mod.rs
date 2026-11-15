//! Registers: small blobs written only by compare-and-swap on a generation.
//!
//! Ported from the Java's `ConditionalBlobContainer`, `BlobRegister` and
//! `BlobRegisterCasResult`
//! (`server/src/main/java/org/opensearch/common/blobstore/ConditionalBlobContainer.java@3920dc36`).
//! A register's object is an 8-byte big-endian generation followed by its
//! value, on every backend, so either side reads what the other wrote. A
//! register starts at a random generation ([`initial_generation`]) and each
//! swap adds one, so no two versions of one register ever have the same
//! bytes, across deletes and re-creates too.
//!
//! Object stores implement the operations here with S3's conditional
//! requests (`tagged`); the filesystem store with the
//! Java's file locks (`crate::fs`).

pub(crate) mod known;
pub(crate) mod tagged;

use crate::chunk::ChunkSource;
use crate::error::{BlobError, BlobResult};

/// The generation of a register that does not exist.
pub const ABSENT_GENERATION: u64 = 0;

/// The longest value a register may hold. Registers are small control
/// blobs; this bounds the copy a write makes of one.
pub const MAX_REGISTER_VALUE: usize = 16 << 20;

/// Bytes of a register's generation, before its value.
pub(crate) const GENERATION_BYTES: usize = 8;

/// What a register read found: the value is in the caller's buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisterRead {
    /// The generation the value was read at, for a later swap.
    pub generation: u64,
    /// The value's length: it fills the front of the buffer.
    pub len: usize,
}

/// How a swap, create or conditional delete ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapOutcome {
    /// The write applied; the register now has this generation.
    Applied(u64),
    /// The write did not apply as the register's next version. The
    /// generation seen instead ([`ABSENT_GENERATION`] for none), or `None`
    /// when the store refused the write without saying what it held. With
    /// `None` after a lost response, this write may have applied and been
    /// replaced by another writer's before the retry was refused, as in the
    /// Java. Either way a later read may see another: read before deciding
    /// again.
    Conflict(Option<u64>),
}

/// The generation a register starts at when it is created: random, from
/// the OS's secure source, in `[2^32, 2^62)` with bit 32 set, as the Java's
/// `BlobRegister.initialGeneration`. Random rather than 1 so a swap
/// carrying a generation read before a delete cannot land on the register
/// created after it; secure rather than seeded so two registers created at
/// once never draw the same one.
pub fn initial_generation() -> BlobResult<u64> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| BlobError::Unavailable)?;
    Ok(from_random(u64::from_be_bytes(bytes)))
}

/// The Java's `1L << 32 | (random & ((1L << 62) - 1))`.
fn from_random(random: u64) -> u64 {
    (1 << 32) | (random & ((1 << 62) - 1))
}

/// The generation after `generation`, refusing one that would leave the
/// Java's signed 64-bit range.
pub(crate) fn next_generation(generation: u64) -> BlobResult<u64> {
    match generation {
        ABSENT_GENERATION => initial_generation(),
        g if g < i64::MAX as u64 => Ok(g.saturating_add(1)),
        _ => Err(BlobError::Protocol("register generation out of range")),
    }
}

/// A register's object as a body: its generation, then `value`, streamed
/// from the caller's slice without a copy of the whole.
pub(crate) struct RegisterSource<'a> {
    header: [u8; GENERATION_BYTES],
    value: &'a [u8],
    pos: usize,
}

impl<'a> RegisterSource<'a> {
    /// The object of `value` at `generation`.
    pub(crate) fn new(generation: u64, value: &'a [u8]) -> Self {
        Self {
            header: generation.to_be_bytes(),
            value,
            pos: 0,
        }
    }
}

impl ChunkSource for RegisterSource<'_> {
    fn len(&self) -> u64 {
        u64::try_from(self.value.len().saturating_add(GENERATION_BYTES)).unwrap_or(u64::MAX)
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let mut n = 0usize;
        while let Some(out) = buf.get_mut(n..).filter(|o| !o.is_empty()) {
            let rest = match self.pos.checked_sub(GENERATION_BYTES) {
                None => self.header.get(self.pos..).unwrap_or_default(),
                Some(at) => self.value.get(at..).unwrap_or_default(),
            };
            if rest.is_empty() {
                break;
            }
            let take = rest.len().min(out.len());
            if let (Some(dst), Some(src)) = (out.get_mut(..take), rest.get(..take)) {
                dst.copy_from_slice(src);
            }
            n = n.saturating_add(take);
            self.pos = self.pos.saturating_add(take);
        }
        Ok(n)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.pos = 0;
        Ok(())
    }
}

/// Refuses a value longer than [`MAX_REGISTER_VALUE`].
pub(crate) fn check_value(value: &[u8]) -> BlobResult<()> {
    if value.len() > MAX_REGISTER_VALUE {
        return Err(BlobError::InvalidArgument(
            "register value longer than 16 MiB",
        ));
    }
    Ok(())
}

/// Splits a register object streamed in chunks: the first 8 bytes are the
/// generation, the rest the value, copied to the front of `out`.
pub(crate) struct Decoder<'a> {
    header: [u8; GENERATION_BYTES],
    seen: usize,
    out: &'a mut [u8],
    len: usize,
}

impl<'a> Decoder<'a> {
    /// A decoder writing the value into `out`.
    pub(crate) fn new(out: &'a mut [u8]) -> Self {
        Self {
            header: [0; GENERATION_BYTES],
            seen: 0,
            out,
            len: 0,
        }
    }

    /// Takes the next chunk of the object.
    pub(crate) fn feed(&mut self, mut chunk: &[u8]) -> BlobResult<()> {
        if self.seen < GENERATION_BYTES {
            let want = GENERATION_BYTES.saturating_sub(self.seen).min(chunk.len());
            let (head, rest) = chunk.split_at(want);
            if let Some(dst) = self
                .header
                .get_mut(self.seen..self.seen.saturating_add(want))
            {
                dst.copy_from_slice(head);
            }
            self.seen = self.seen.saturating_add(want);
            chunk = rest;
        }
        let end = self.len.saturating_add(chunk.len());
        let dst = self
            .out
            .get_mut(self.len..end)
            .ok_or(BlobError::InvalidArgument(
                "register larger than the buffer",
            ))?;
        dst.copy_from_slice(chunk);
        self.len = end;
        Ok(())
    }

    /// The register read, or `None` for an empty object, which the Java's
    /// filesystem store reads as absent.
    pub(crate) fn finish(&self) -> BlobResult<Option<RegisterRead>> {
        match self.seen {
            0 => Ok(None),
            GENERATION_BYTES => Ok(Some(RegisterRead {
                generation: u64::from_be_bytes(self.header),
                len: self.len,
            })),
            _ => Err(BlobError::Protocol("register shorter than its generation")),
        }
    }
}

/// Reads the generation from the first bytes of a register object.
pub(crate) fn generation_of(prefix: &[u8]) -> BlobResult<u64> {
    let bytes: [u8; GENERATION_BYTES] = prefix
        .get(..GENERATION_BYTES)
        .and_then(|b| b.try_into().ok())
        .ok_or(BlobError::Protocol("register shorter than its generation"))?;
    Ok(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_generations_are_in_the_javas_range() {
        for random in [0, u64::MAX, 1 << 62, (1 << 32) - 1] {
            let g = from_random(random);
            assert!((1 << 32..1 << 62).contains(&g), "{g:#x}");
            assert_ne!(g & (1 << 32), 0);
        }
        assert_eq!(from_random(0), 1 << 32);
        assert_eq!(from_random(u64::MAX), (1 << 62) - 1);
        let a = initial_generation().unwrap();
        let b = initial_generation().unwrap();
        assert_ne!(a, b);
        assert!((1 << 32..1 << 62).contains(&a));
    }

    #[test]
    fn a_generation_steps_by_one_within_the_signed_range() {
        assert_eq!(next_generation(5), Ok(6));
        assert_eq!(next_generation(i64::MAX as u64 - 1), Ok(i64::MAX as u64));
        assert!(next_generation(i64::MAX as u64).is_err());
        assert!(next_generation(ABSENT_GENERATION).unwrap() >= 1 << 32);
    }

    #[test]
    fn a_register_object_is_its_generation_then_its_value() {
        let body = encode(0x0102_0304_0506_0708, b"value");
        assert_eq!(&body[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&body[8..], b"value");
        assert_eq!(generation_of(&body), Ok(0x0102_0304_0506_0708));
        assert!(generation_of(&body[..7]).is_err());
        assert!(check_value(&vec![0; MAX_REGISTER_VALUE]).is_ok());
        assert!(check_value(&vec![0; MAX_REGISTER_VALUE + 1]).is_err());
    }

    /// The whole object a source yields, through a buffer of `size`.
    fn drain(source: &mut RegisterSource<'_>, size: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; size];
        loop {
            let n = source.fill(&mut buf).unwrap();
            if n == 0 {
                return out;
            }
            assert!(
                n == size || source.fill(&mut buf).unwrap() == 0,
                "a short fill ends the body"
            );
            out.extend_from_slice(&buf[..n]);
        }
    }

    fn encode(generation: u64, value: &[u8]) -> Vec<u8> {
        drain(&mut RegisterSource::new(generation, value), 64)
    }

    #[test]
    fn a_source_yields_the_object_through_any_buffer_and_rewinds() {
        let value = b"hello, register";
        let whole = [&42u64.to_be_bytes()[..], value].concat();
        for size in 1..=whole.len() + 1 {
            let mut source = RegisterSource::new(42, value);
            assert_eq!(source.len(), whole.len() as u64);
            assert_eq!(drain(&mut source, size), whole, "{size}");
            source.rewind().unwrap();
            assert_eq!(drain(&mut source, size), whole, "{size} rewound");
        }
        let mut empty = RegisterSource::new(7, b"");
        assert_eq!(drain(&mut empty, 3), 7u64.to_be_bytes());
    }

    #[test]
    fn a_decoder_splits_any_chunking() {
        let body = encode(42, b"hello world");
        for size in 1..body.len() {
            let mut out = [0u8; 32];
            let mut d = Decoder::new(&mut out);
            for chunk in body.chunks(size) {
                d.feed(chunk).unwrap();
            }
            let read = d.finish().unwrap().unwrap();
            assert_eq!(read.generation, 42);
            assert_eq!(&out[..read.len], b"hello world");
        }
    }

    #[test]
    fn a_decoder_refuses_what_does_not_fit_or_is_too_short() {
        let body = encode(1, b"too long");
        let mut out = [0u8; 4];
        let mut d = Decoder::new(&mut out);
        assert!(matches!(d.feed(&body), Err(BlobError::InvalidArgument(_))));
        let mut out = [0u8; 4];
        let mut d = Decoder::new(&mut out);
        d.feed(&[1, 2, 3]).unwrap();
        assert!(matches!(d.finish(), Err(BlobError::Protocol(_))));
        let mut out = [0u8; 4];
        assert_eq!(Decoder::new(&mut out).finish(), Ok(None));
    }
}
