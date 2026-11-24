//! The value of a snapshot's name register (`snapshot-states/<sha256>`), as
//! the shell's `SnapshotStore.encode` writes it: the discovery marker, the
//! record and a finished flag, in the Java's stream encoding (a variable
//! length count, then the bytes; a boolean is one byte).
//!
//! The marker is ASCII by construction (a hash, `#` and an id), and a string's
//! length in the Java's stream is its character count, so a marker that is
//! not ASCII is refused rather than read as the wrong length.

use crate::error::{FormatError, corrupt};
use crate::snapshot_pins::MAX_SNAPSHOT_RECORD_BYTES;

const WHAT: &str = "snapshot state";

/// The value a deleted snapshot's register is left with.
const RELEASED: &[u8] = br#"{"_released":true}"#;

/// What a name register holds.
#[derive(Debug, PartialEq, Eq)]
pub enum SnapshotState<'a> {
    /// The name's snapshot was deleted: no snapshot has this name now.
    Released,
    /// A snapshot.
    Held {
        /// The discovery marker blob naming this lifetime.
        marker: &'a str,
        /// The snapshot record's JSON.
        record: &'a [u8],
        /// Whether its capture finished.
        finished: bool,
    },
}

/// A variable-length count: seven bits a byte, low first, at most five bytes.
fn vint(bytes: &[u8], at: &mut usize) -> Result<usize, FormatError> {
    let mut value: u32 = 0;
    for shift in (0..35).step_by(7) {
        let b = *bytes
            .get(*at)
            .ok_or_else(|| corrupt(WHAT, "ends inside a count"))?;
        *at = at.saturating_add(1);
        value |= u32::from(b & 0x7f)
            .checked_shl(shift)
            .ok_or_else(|| corrupt(WHAT, "a count is too long"))?;
        if b & 0x80 == 0 {
            return usize::try_from(value).map_err(|_| corrupt(WHAT, "a count is too long"));
        }
    }
    Err(corrupt(WHAT, "a count is too long"))
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, n: usize) -> Result<&'a [u8], FormatError> {
    let end = at
        .checked_add(n)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| corrupt(WHAT, "ends inside a field"))?;
    let out = bytes.get(*at..end).unwrap_or_default();
    *at = end;
    Ok(out)
}

impl<'a> SnapshotState<'a> {
    /// Reads a name register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a value that ends early, has bytes left
    /// over, has a non-ASCII marker or a boolean that is not 0 or 1, or
    /// holds a record past [`MAX_SNAPSHOT_RECORD_BYTES`].
    pub fn parse(value: &'a [u8]) -> Result<Self, FormatError> {
        if value == RELEASED {
            return Ok(Self::Released);
        }
        let mut at = 0;
        let marker_len = vint(value, &mut at)?;
        let marker = take(value, &mut at, marker_len)?;
        if !marker.is_ascii() {
            return Err(corrupt(WHAT, "a marker that is not ASCII"));
        }
        let marker =
            std::str::from_utf8(marker).map_err(|_| corrupt(WHAT, "a marker that is not text"))?;
        let record_len = vint(value, &mut at)?;
        if record_len > MAX_SNAPSHOT_RECORD_BYTES {
            return Err(corrupt(WHAT, "a record past the size limit"));
        }
        let record = take(value, &mut at, record_len)?;
        let finished = match take(value, &mut at, 1)? {
            [0] => false,
            [1] => true,
            _ => return Err(corrupt(WHAT, "a finished flag that is not a boolean")),
        };
        if at != value.len() {
            return Err(corrupt(WHAT, "bytes after the finished flag"));
        }
        Ok(Self::Held {
            marker,
            record,
            finished,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(marker: &str, record: &[u8], finished: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for part in [marker.as_bytes(), record] {
            let mut n = part.len();
            while n >= 0x80 {
                out.push(u8::try_from(n & 0x7f).unwrap() | 0x80);
                n >>= 7;
            }
            out.push(u8::try_from(n).unwrap());
            out.extend_from_slice(part);
        }
        out.push(u8::from(finished));
        out
    }

    #[test]
    fn a_held_snapshot_reads_with_its_marker_record_and_flag() {
        let record = vec![b'x'; 300];
        let bytes = encode("v2-ab#cd#ef", &record, true);
        assert_eq!(
            SnapshotState::parse(&bytes).expect("reads"),
            SnapshotState::Held {
                marker: "v2-ab#cd#ef",
                record: &record,
                finished: true
            }
        );
        let open = encode("m", b"{}", false);
        assert!(matches!(
            SnapshotState::parse(&open).expect("reads"),
            SnapshotState::Held {
                finished: false,
                ..
            }
        ));
    }

    #[test]
    fn the_released_value_is_no_snapshot() {
        assert_eq!(
            SnapshotState::parse(br#"{"_released":true}"#).expect("reads"),
            SnapshotState::Released
        );
    }

    #[test]
    fn a_value_that_is_cut_padded_or_odd_is_refused() {
        let good = encode("m", b"{}", true);
        let mut padded = good.clone();
        padded.push(0);
        let mut flag = good.clone();
        *flag.last_mut().unwrap() = 2;
        let mut non_ascii = encode("m", b"{}", true);
        non_ascii[1] = 0xc3;
        let long_count = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        for bad in [
            &good[..good.len() - 1],
            &good[..1],
            &padded[..],
            &flag[..],
            &non_ascii[..],
            &long_count[..],
            b"",
        ] {
            assert!(SnapshotState::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_record_past_the_limit_is_refused_before_it_is_taken() {
        let mut bytes = encode("m", b"", true);
        // Replace the empty record's count with one past the limit.
        let past = MAX_SNAPSHOT_RECORD_BYTES + 1;
        let mut count = Vec::new();
        let mut n = past;
        while n >= 0x80 {
            count.push(u8::try_from(n & 0x7f).unwrap() | 0x80);
            n >>= 7;
        }
        count.push(u8::try_from(n).unwrap());
        bytes.splice(2..3, count);
        assert!(SnapshotState::parse(&bytes).is_err());
    }
}
