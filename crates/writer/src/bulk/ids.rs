//! `UUIDs.base64UUID`: the ids OpenSearch gives an `index` or `create`
//! without one, laid out as `TimeBasedUUIDGenerator` lays them out so that
//! they sort and compress in the terms dictionary as the Java's do.

/// Gives time-based ids: 15 bytes of sequence, time and node address,
/// base64url without padding (20 characters).
#[derive(Debug)]
pub struct IdGenerator {
    sequence: u32,
    last_millis: u64,
    address: [u8; 6],
}

impl IdGenerator {
    /// A generator whose sequence starts at `seed` and whose ids carry
    /// `address` (`MacAddressProvider.getSecureMungedAddress`).
    pub fn new(seed: u32, address: [u8; 6]) -> Self {
        Self {
            sequence: seed,
            last_millis: 0,
            address,
        }
    }

    /// A generator seeded from the process's random hash keys.
    pub fn random() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(0);
        Self::from_bits(h.finish().to_le_bytes())
    }

    /// A generator from eight random bytes: four seed the sequence, and
    /// the address is made of the other four and two munged from the first
    /// two, with the multicast bit set as the Java sets it on a random
    /// address.
    fn from_bits(bits: [u8; 8]) -> Self {
        let [a, b, c, d, e, f, g, i] = bits;
        Self::new(
            u32::from_le_bytes([a, b, c, d]),
            [e | 1, f, g, i, a ^ e, b ^ f],
        )
    }

    /// The next id, at `now_millis`.
    pub fn next(&mut self, now_millis: u64) -> String {
        self.sequence = self.sequence.wrapping_add(1);
        let sequence = self.sequence & 0x00ff_ffff;
        let mut timestamp = self.last_millis.max(now_millis);
        if sequence == 0 {
            // The Java forces the clock on whenever the sequence wraps.
            timestamp = timestamp.saturating_add(1);
        }
        self.last_millis = timestamp;
        let t = timestamp.to_le_bytes();
        let s = sequence.to_le_bytes();
        let m = self.address;
        let bytes = [
            s[0], s[2], t[2], t[3], t[4], t[5], m[0], m[1], m[2], m[3], m[4], m[5], t[1], s[1],
            t[0],
        ];
        base64url(&bytes)
    }
}

/// Base64 with the URL alphabet and no padding, of 15 bytes (20
/// characters).
fn base64url(bytes: &[u8; 15]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    // ALLOC: one 20-character id an item without one.
    let mut out = String::with_capacity(20);
    for chunk in bytes.chunks_exact(3) {
        let [a, b, c] = [chunk[0], chunk[1], chunk[2]];
        let n = u32::from_be_bytes([0, a, b, c]);
        for shift in [18u32, 12, 6, 0] {
            let index = usize::try_from((n >> shift) & 63).unwrap_or(0);
            out.push(char::from(ALPHABET[index]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_laid_out_as_the_javas() {
        // Bytes as TimeBasedUUIDGenerator places them for sequence
        // 0x010203, timestamp 0x0000_0a0b_0c0d_0e0f and address 1..=6.
        let mut g = IdGenerator::new(0x0001_0202, [1, 2, 3, 4, 5, 6]);
        let id = g.next(0x0a0b_0c0d_0e0f);
        let expected = base64url(&[
            0x03, 0x01, 0x0d, 0x0c, 0x0b, 0x0a, 1, 2, 3, 4, 5, 6, 0x0e, 0x02, 0x0f,
        ]);
        assert_eq!(id, expected);
        assert_eq!(id.len(), 20);
        // The clock never goes back.
        let later = g.next(5);
        assert_ne!(later, id);
        assert_eq!(g.last_millis, 0x0a0b_0c0d_0e0f);
    }

    #[test]
    fn a_wrapped_sequence_moves_the_clock_on() {
        let mut g = IdGenerator::new(0x00ff_ffff, [0; 6]);
        g.next(100);
        assert_eq!(g.last_millis, 101);
        g.next(100);
        assert_eq!(g.last_millis, 101);
    }

    #[test]
    fn base64url_uses_the_url_alphabet() {
        assert_eq!(base64url(&[0xfb; 15]), "-_v7-_v7-_v7-_v7-_v7");
        assert_eq!(base64url(&[0; 15]), "AAAAAAAAAAAAAAAAAAAA");
    }

    #[test]
    fn a_random_address_is_munged_from_the_bits() {
        let g = IdGenerator::from_bits([0x13, 0x26, 0x30, 0x40, 0x05, 0x0c, 0x06, 0x07]);
        assert_eq!(g.sequence, 0x4030_2613);
        assert_eq!(g.address, [0x05, 0x0c, 0x06, 0x07, 0x16, 0x2a]);
    }

    #[test]
    fn random_generators_differ() {
        let (mut a, mut b) = (IdGenerator::random(), IdGenerator::random());
        assert_ne!(a.next(1), b.next(1));
    }
}
