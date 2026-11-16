//! The 64-bit routing hash and the declared-shard slices of its space
//! (`RoutingHash`): murmur3 x64 128-bit over the routing value's UTF-8,
//! seed 0, taking `h1` as an unsigned point in `[0, 2^64)`.

const C1: u64 = 0x87c3_7b91_1142_53d5;
const C2: u64 = 0x4cf5_ad43_2745_937f;

/// The hash of a routing value: the document's routing when it has one,
/// its id otherwise (`RoutingHash.of`).
pub fn routing_hash(routing: &str) -> u64 {
    murmur3_x64_h1(routing.as_bytes())
}

/// `h1` of murmur3 x64 128-bit with seed 0, as OpenSearch's
/// `MurmurHash3.hash128`.
pub fn murmur3_x64_h1(key: &[u8]) -> u64 {
    let mut h1: u64 = 0;
    let mut h2: u64 = 0;
    let mut blocks = key.chunks_exact(16);
    for block in &mut blocks {
        let (low, high) = block.split_at(8);
        let k1 = mix_k1(le(low));
        h1 ^= k1;
        h1 = h1.rotate_left(27).wrapping_add(h2);
        h1 = h1.wrapping_mul(5).wrapping_add(0x52dc_e729);
        h2 ^= mix_k2(le(high));
        h2 = h2.rotate_left(31).wrapping_add(h1);
        h2 = h2.wrapping_mul(5).wrapping_add(0x3849_5ab5);
    }
    // The Java mixes a tail only when it has bytes; a missing part reads
    // as zero, which mixes to zero, so mixing it always is the same.
    let tail = blocks.remainder();
    h2 ^= mix_k2(le(tail.get(8..).unwrap_or_default()));
    h1 ^= mix_k1(le(tail.get(..tail.len().min(8)).unwrap_or_default()));
    let len = key.len() as u64;
    h1 ^= len;
    h2 ^= len;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    h1 = fmix(h1);
    h2 = fmix(h2);
    h1.wrapping_add(h2)
}

/// Up to eight bytes as a little-endian number, missing high bytes zero.
fn le(bytes: &[u8]) -> u64 {
    let mut word = [0u8; 8];
    for (slot, b) in word.iter_mut().zip(bytes) {
        *slot = *b;
    }
    u64::from_le_bytes(word)
}

fn mix_k1(k: u64) -> u64 {
    k.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2)
}

fn mix_k2(k: u64) -> u64 {
    k.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1)
}

fn fmix(mut k: u64) -> u64 {
    k ^= k >> 33;
    k = k.wrapping_mul(0xff51_afd7_ed55_8ccd);
    k ^= k >> 33;
    k = k.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    k ^ (k >> 33)
}

/// The declared shard a hash falls in: the `i` with
/// `i * 2^64 / n <= hash < (i + 1) * 2^64 / n` (`RoutingHash.declaredShard`).
/// `n` must be positive.
pub fn declared_shard(hash: u64, declared_shards: u32) -> u32 {
    let high = u128::from(hash)
        .wrapping_mul(u128::from(declared_shards))
        .wrapping_shr(64);
    // Below `declared_shards`, so it fits.
    u32::try_from(high).unwrap_or(u32::MAX)
}

/// The first hash of a declared shard, `ceil(shard * 2^64 / n)`: the
/// smallest hash [`declared_shard`] places in `shard`. `None` when `shard`
/// is not below `declared_shards`.
pub fn first_hash_of(shard: u32, declared_shards: u32) -> Option<u64> {
    if shard >= declared_shards {
        return None;
    }
    if shard == 0 {
        return Some(0);
    }
    let numerator = u128::from(shard).wrapping_shl(64).wrapping_sub(1);
    let first = numerator
        .checked_div(u128::from(declared_shards))?
        .wrapping_add(1);
    // Below 2^64 because shard < declared_shards.
    u64::try_from(first).ok()
}

/// The last hash of a declared shard, inclusive. `None` when `shard` is
/// not below `declared_shards`.
pub fn last_hash_of(shard: u32, declared_shards: u32) -> Option<u64> {
    if shard >= declared_shards {
        return None;
    }
    if shard.saturating_add(1) == declared_shards {
        return Some(u64::MAX);
    }
    first_hash_of(shard.saturating_add(1), declared_shards).map(|next| next.saturating_sub(1))
}
