//! The two string hashes placement agrees on with the Java: OpenSearch's
//! `Murmur3HashFunction.hash(String)` (Lucene's 32-bit murmur3 over each
//! UTF-16 unit as two little-endian bytes, seed 0), and Java's
//! `String.hashCode`. Both take a key in parts, so `node/index#shard` is
//! hashed without being built.

/// A shard of an index, as placement keys it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardKey<'a> {
    /// The index name.
    pub index: &'a str,
    /// The shard number.
    pub shard: i32,
}

impl<'a> ShardKey<'a> {
    /// The shard `shard` of `index`.
    pub fn new(index: &'a str, shard: i32) -> Self {
        Self { index, shard }
    }
}

/// Java's `Integer.toString` of `n`, into `buf`; the digits it wrote.
pub(crate) fn decimal(n: i32, buf: &mut [u8; 11]) -> &str {
    let mut at = buf.len();
    let mut rest = n.unsigned_abs();
    loop {
        at = at.saturating_sub(1);
        // A digit is below 10, so the cast cannot lose anything.
        buf[at] = b'0'.saturating_add(u8::try_from(rest % 10).unwrap_or(0));
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    if n < 0 {
        at = at.saturating_sub(1);
        buf[at] = b'-';
    }
    std::str::from_utf8(&buf[at..]).unwrap_or_default()
}

/// Murmur3 (x86, 32 bits, seed 0) of `parts` joined, each UTF-16 unit as
/// its low byte then its high byte, as `Murmur3HashFunction.hash(String)`.
pub fn murmur3(parts: &[&str]) -> i32 {
    let mut m = Murmur3::default();
    for part in parts {
        for unit in part.encode_utf16() {
            let [low, high] = unit.to_le_bytes();
            m.push(low);
            m.push(high);
        }
    }
    m.finish()
}

/// Java's `String.hashCode` of `parts` joined.
pub fn java_hash_code(parts: &[&str]) -> i32 {
    parts
        .iter()
        .flat_map(|p| p.encode_utf16())
        .fold(0i32, |h, unit| {
            h.wrapping_mul(31).wrapping_add(i32::from(unit))
        })
}

const C1: u32 = 0xcc9e_2d51;
const C2: u32 = 0x1b87_3593;

#[derive(Default)]
struct Murmur3 {
    h: u32,
    block: u32,
    filled: u32,
    len: u32,
}

impl Murmur3 {
    fn push(&mut self, byte: u8) {
        self.block |= u32::from(byte).wrapping_shl(self.filled.wrapping_mul(8));
        self.filled = self.filled.wrapping_add(1);
        self.len = self.len.wrapping_add(1);
        if self.filled == 4 {
            self.h ^= mix(self.block);
            self.h = self
                .h
                .rotate_left(13)
                .wrapping_mul(5)
                .wrapping_add(0xe654_6b64);
            self.block = 0;
            self.filled = 0;
        }
    }

    fn finish(self) -> i32 {
        let mut h = self.h;
        if self.filled > 0 {
            h ^= mix(self.block);
        }
        h ^= self.len;
        h ^= h >> 16;
        h = h.wrapping_mul(0x85eb_ca6b);
        h ^= h >> 13;
        h = h.wrapping_mul(0xc2b2_ae35);
        h ^= h >> 16;
        i32::from_ne_bytes(h.to_ne_bytes())
    }
}

fn mix(k: u32) -> u32 {
    k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2)
}

#[cfg(test)]
mod tests {
    use super::decimal;

    #[test]
    fn decimal_is_javas_integer_to_string() {
        let mut buf = [0u8; 11];
        for (n, s) in [
            (0, "0"),
            (7, "7"),
            (41, "41"),
            (-5, "-5"),
            (i32::MAX, "2147483647"),
            (i32::MIN, "-2147483648"),
        ] {
            assert_eq!(decimal(n, &mut buf), s);
        }
    }
}
