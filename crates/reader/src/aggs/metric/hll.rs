//! A `cardinality` aggregation's shard-level sketch: OpenSearch's
//! `HyperLogLogPlusPlus` for one bucket, read from the bytes
//! `AbstractHyperLogLogPlusPlus.writeTo` writes (what lucene-aggs builds),
//! with its estimate (`AbstractLinearCounting.cardinality` and
//! `AbstractHyperLogLog.cardinality`) and its merge
//! (`HyperLogLogPlusPlus.merge`), ported from
//! server/src/main/java/org/opensearch/search/aggregations/metrics/ at
//! OpenSearch 3.10.0.
//!
//! Until it holds more than three quarters of `2^p / 4` hashes, a sketch is
//! linear counting over hashes encoded with 25 bits of index; past that it is
//! `2^p` HyperLogLog registers, and its estimate is bias-corrected with
//! OpenSearch's tables (`hll_bias.txt`).

use std::sync::OnceLock;

/// `P2`: the bits of index an encoded hash keeps.
const P2: u32 = 25;

/// `BIAS_K`: how many neighbouring raw estimates weigh a bias.
const BIAS_K: usize = 6;

/// One bucket's sketch at precision `p` (4 to 18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hll {
    p: u32,
    repr: Repr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Repr {
    /// `LinearCounting`'s open-addressing table of encoded hashes, `2^p / 4`
    /// slots, 0 for empty, and how many it holds.
    Linear { slots: Vec<i32>, size: usize },
    /// `HyperLogLog`'s run length per register.
    Registers(Vec<u8>),
}

/// The most bytes one `cardinality` at precision `p` holds: lucene-aggs'
/// sketch and this one, each a table or registers of `2^p` bytes and both
/// while upgrading, and the written form between them, `2^p` and a header.
pub fn sketch_bytes(p: u32) -> usize {
    (1usize << p.min(18)).saturating_mul(5).saturating_add(16)
}

impl Hll {
    /// The sketch `AbstractHyperLogLogPlusPlus.writeTo` wrote into `bytes`:
    /// `vint p`, a boolean byte (true for HyperLogLog), then a `vlong` count
    /// and big-endian encoded hashes, or the `2^p` register bytes. `None`
    /// for bytes it cannot have written.
    pub fn read(bytes: &[u8]) -> Option<Hll> {
        let (&p, rest) = bytes.split_first()?;
        let p = u32::from(p);
        if !(4..=18).contains(&p) {
            return None;
        }
        let (&algorithm, rest) = rest.split_first()?;
        let mut hll = Hll::empty(p);
        match algorithm {
            0 => {
                let (count, rest) = read_vlong(rest)?;
                let count = usize::try_from(count).ok()?;
                if rest.len() != count.checked_mul(4)? {
                    return None;
                }
                for chunk in rest.chunks_exact(4) {
                    let encoded = i32::from_be_bytes(chunk.try_into().ok()?);
                    if encoded == 0 {
                        return None;
                    }
                    hll.add_encoded(encoded);
                }
            }
            1 => {
                if rest.len() != 1usize << p {
                    return None;
                }
                // ALLOC: `2^p` bytes, at most 256 KiB, reserved by the caller.
                hll.repr = Repr::Registers(rest.to_vec());
            }
            _ => return None,
        }
        Some(hll)
    }

    /// An empty sketch at `p`, counting linearly.
    fn empty(p: u32) -> Hll {
        Hll {
            p,
            repr: Repr::Linear {
                // ALLOC: `2^p / 4` ints, `2^p` bytes, reserved by the caller.
                slots: vec![0; (1usize << p) / 4],
                size: 0,
            },
        }
    }

    /// An empty sketch of `other`'s precision, as a reduce starts.
    pub fn empty_like(other: &Hll) -> Hll {
        Hll::empty(other.p)
    }

    /// The precision.
    pub fn precision(&self) -> u32 {
        self.p
    }

    /// `LinearCounting.threshold`: `(int) (capacity * 0.75f)`.
    fn threshold(&self) -> usize {
        ((1usize << self.p) / 4).saturating_mul(3) / 4
    }

    /// `addEncoded`, then `upgradeToHll` when the table passes its threshold;
    /// `collectEncoded` once upgraded.
    fn add_encoded(&mut self, encoded: i32) {
        let threshold = self.threshold();
        let p = self.p;
        match &mut self.repr {
            Repr::Linear { slots, size } => {
                let mask = slots.len().saturating_sub(1);
                // ARITH: none; the bits of an int, as Java's `encoded & mask`.
                #[allow(clippy::cast_sign_loss)]
                let mut i = (encoded as u32 as usize) & mask;
                loop {
                    let Some(slot) = slots.get_mut(i) else { return };
                    if *slot == 0 {
                        *slot = encoded;
                        *size = size.saturating_add(1);
                        break;
                    }
                    if *slot == encoded {
                        return;
                    }
                    i = i.wrapping_add(1) & mask;
                }
                if *size > threshold {
                    self.upgrade();
                }
            }
            Repr::Registers(runs) => {
                add_run_len(runs, decode_index(encoded, p), decode_run_len(encoded, p));
            }
        }
    }

    /// `upgradeToHll`: the table's hashes, in slot order, into registers.
    fn upgrade(&mut self) {
        if let Repr::Linear { slots, .. } = &self.repr {
            // ALLOC: `2^p` bytes, reserved by the caller.
            let mut runs = vec![0u8; 1usize << self.p];
            for &encoded in slots.iter().filter(|&&e| e != 0) {
                add_run_len(
                    &mut runs,
                    decode_index(encoded, self.p),
                    decode_run_len(encoded, self.p),
                );
            }
            self.repr = Repr::Registers(runs);
        }
    }

    /// `HyperLogLogPlusPlus.merge(0, other, 0)`. `false`, and nothing
    /// merged, when the precisions differ, which OpenSearch refuses.
    pub fn merge(&mut self, other: &Hll) -> bool {
        if self.p != other.p {
            return false;
        }
        match &other.repr {
            Repr::Linear { slots, .. } => {
                for &encoded in slots.iter().filter(|&&e| e != 0) {
                    self.add_encoded(encoded);
                }
            }
            Repr::Registers(theirs) => {
                self.upgrade();
                if let Repr::Registers(runs) = &mut self.repr {
                    for (i, &run) in theirs.iter().enumerate() {
                        add_run_len(runs, i, run);
                    }
                }
            }
        }
        true
    }

    /// `cardinality(0)`: the estimate of how many distinct values went in.
    pub fn cardinality(&self) -> u64 {
        match &self.repr {
            Repr::Linear { size, .. } => {
                let m = 1u64 << P2;
                linear_counting(m, m.saturating_sub(*size as u64))
            }
            Repr::Registers(runs) => hll_cardinality(self.p, runs),
        }
    }
}

/// `addRunLen`: a register keeps its longest run.
fn add_run_len(runs: &mut [u8], register: usize, run_len: u8) {
    if let Some(r) = runs.get_mut(register) {
        *r = (*r).max(run_len);
    }
}

/// `decodeIndex`.
fn decode_index(encoded: i32, p: u32) -> usize {
    // ARITH: none; Java's unsigned shifts of the int's bits.
    #[allow(clippy::cast_sign_loss)]
    let bits = encoded as u32;
    let index = if bits & 1 == 1 { bits >> 7 } else { bits >> 1 };
    (index >> P2.saturating_sub(p)) as usize
}

/// `decodeRunLen`.
fn decode_run_len(encoded: i32, p: u32) -> u8 {
    // ARITH: none; Java's int shifts of the int's bits, and a run length of
    // at most `64 - p + 1`, which fits a byte.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    {
        let bits = encoded as u32;
        if bits & 1 == 1 {
            ((bits >> 1) & 0x3F).saturating_add(P2.saturating_sub(p)) as u8
        } else {
            let shifted = bits << (p.saturating_add(31).saturating_sub(P2) & 31);
            shifted.leading_zeros().saturating_add(1) as u8
        }
    }
}

/// `AbstractCardinalityAlgorithm.linearCounting`:
/// `Math.round(m * Math.log((double) m / v))`.
fn linear_counting(m: u64, v: u64) -> u64 {
    // ARITH: none; `m` is at most 2^25 and `v` at most `m`, both exact as
    // doubles; the estimate is non-negative, and `as` saturates as Java's
    // `Math.round` does.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    {
        let m = m as f64;
        java_round(m * (m / v as f64).ln()) as u64
    }
}

/// `Math.round(double)` of a non-negative estimate: the closest whole
/// number, ties up, which for `x >= 0` is Rust's ties-away rounding.
fn java_round(x: f64) -> f64 {
    x.round()
}

/// `AbstractHyperLogLog.cardinality` over the registers `runs`.
fn hll_cardinality(p: u32, runs: &[u8]) -> u64 {
    let m = 1u64 << p;
    // ARITH: none; `m` is at most 2^18, exact as a double.
    #[allow(clippy::cast_precision_loss)]
    let mf = m as f64;
    let alpha = match p {
        4 => 0.673,
        5 => 0.697,
        _ => 0.7213 / (1.0 + 1.079 / mf),
    };
    let alpha_mm = alpha * mf * mf;
    let mut inverse_sum = 0.0f64;
    let mut zeros = 0u64;
    for &run in runs {
        // `1. / (1L << runLen)`: a run is at most 61, well inside a long.
        // ARITH: none; a power of two below 2^64 is exact as a double.
        #[allow(clippy::cast_precision_loss)]
        let power = (1u64 << run.min(63)) as f64;
        inverse_sum += 1.0 / power;
        if run == 0 {
            zeros = zeros.saturating_add(1);
        }
    }
    let e1 = alpha_mm / inverse_sum;
    let e2 = if e1 <= 5.0 * mf {
        e1 - estimate_bias(p, e1)
    } else {
        e1
    };
    // ARITH: none; `as` saturates as `Math.round`'s long does, and the
    // estimates are positive.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rounded = java_round(e2) as u64;
    let h = if zeros != 0 {
        linear_counting(m, zeros)
    } else {
        rounded
    };
    if h <= tables().thresholds.get(row(p)).copied().unwrap_or(0) {
        h
    } else {
        rounded
    }
}

/// The table row of precision `p`.
fn row(p: u32) -> usize {
    p.saturating_sub(4) as usize
}

/// `estimateBias(e)`: the bias of the `BIAS_K` raw estimates nearest `e`,
/// weighted by their inverse distance.
fn estimate_bias(p: u32, e: f64) -> f64 {
    let t = tables();
    let (Some(raw), Some(bias)) = (t.raw.get(row(p)), t.bias.get(row(p))) else {
        return 0.0;
    };
    let mut weights = [0.0f64; BIAS_K];
    let mut index = bias.len().saturating_sub(BIAS_K);
    for (i, &r) in raw.iter().enumerate() {
        let w = 1.0 / (r - e).abs();
        let Some(slot) = weights.get_mut(i % BIAS_K) else {
            break;
        };
        if w.is_infinite() {
            return bias.get(i).copied().unwrap_or(0.0);
        } else if *slot >= w {
            index = i.wrapping_sub(BIAS_K);
            break;
        }
        *slot = w;
    }
    let mut weight_sum = 0.0;
    let mut bias_sum = 0.0;
    for (i, &w) in weights.iter().enumerate() {
        let b = bias.get(index.wrapping_add(i)).copied().unwrap_or(0.0);
        bias_sum += w * b;
        weight_sum += w;
    }
    bias_sum / weight_sum
}

/// `THRESHOLDS`, `RAW_ESTIMATE_DATA` and `BIAS_DATA`, a row per precision.
struct Tables {
    thresholds: Vec<u64>,
    raw: Vec<Vec<f64>>,
    bias: Vec<Vec<f64>>,
}

/// The tables, read once from `hll_bias.txt`.
fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        // ALLOC: once per process; 15 rows of about 200 doubles each.
        let (thresholds, raw, bias) = (Vec::new(), Vec::new(), Vec::new());
        let mut t = Tables {
            thresholds,
            raw,
            bias,
        };
        let numbers = |s: &str| -> Vec<f64> {
            // ALLOC: once per process, as above.
            s.split_whitespace()
                .filter_map(|n| n.parse().ok())
                .collect()
        };
        for line in include_str!("hll_bias.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
        {
            // ALLOC: none; an iterator over the line.
            let mut parts = line.split('|');
            t.thresholds
                .push(parts.next().and_then(|s| s.parse().ok()).unwrap_or(0));
            t.raw.push(numbers(parts.next().unwrap_or_default()));
            t.bias.push(numbers(parts.next().unwrap_or_default()));
        }
        t
    })
}

/// A `vlong`: seven bits a byte, low first, the high bit set while more
/// follow.
fn read_vlong(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let mut value = 0u64;
    for (i, &b) in bytes.iter().enumerate().take(10) {
        let shift = u32::try_from(i).ok()?.checked_mul(7)?;
        value |= u64::from(b & 0x7F).checked_shl(shift)?;
        if b & 0x80 == 0 {
            return Some((value, bytes.get(i.checked_add(1)?..)?));
        }
    }
    None
}

/// `HyperLogLogPlusPlus.precisionFromThreshold(count)`, or the message of
/// the `IllegalArgumentException` `PackedInts.bitsRequired` throws when
/// `4 * ceil(count / 0.75f)` overflows a long negative.
pub fn precision_from_threshold(count: i64) -> Result<u32, String> {
    // ARITH: none; Java divides the long as a float, rounds up as a double
    // and casts back with `(long)`, which saturates as `as` does.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let entries = (f64::from(count as f32 / 0.75f32)).ceil() as i64;
    let bytes = entries.wrapping_mul(4);
    if bytes < 0 {
        // ALLOC: error path; one message.
        return Err(format!("maxValue must be non-negative (got: {bytes})"));
    }
    // `PackedInts.bitsRequired`: `max(1, 64 - numberOfLeadingZeros)`.
    let bits = 64u32.saturating_sub(bytes.leading_zeros()).max(1);
    Ok(bits.clamp(4, 18))
}

#[cfg(test)]
#[path = "hll_tests.rs"]
mod tests;
