//! A bounded uniform sample of one field's vectors, drawn while a shard's
//! segments are read once: what training a k-NN model learns from
//! (ADR 0087 decision 5).

use crate::OpenShard;
use crate::failure::Failure;

/// A reservoir of at most `cap` vectors of `dimension` floats: Algorithm R,
/// so every vector offered has the same chance of being kept.
pub struct Reservoir {
    dimension: usize,
    cap: usize,
    seen: u64,
    state: u64,
    data: Vec<f32>,
}

impl Reservoir {
    /// A reservoir for `cap` vectors, drawing with `seed`.
    pub fn new(dimension: usize, cap: usize, seed: u64) -> Self {
        Self {
            dimension,
            cap,
            seen: 0,
            state: seed,
            // ALLOC: cap x dimension floats, which the caller reserved from the training pool.
            data: Vec::with_capacity(cap.saturating_mul(dimension)),
        }
    }

    /// The next draw below `n`: SplitMix64, scaled without a modulo.
    fn below(&mut self, n: u64) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        u64::try_from(u128::from(z).saturating_mul(u128::from(n)) >> 64).unwrap_or(0)
    }

    /// Offers one vector.
    pub fn offer(&mut self, vector: &[f32]) {
        let before = self.seen;
        self.seen = self.seen.saturating_add(1);
        let kept = usize::try_from(before).unwrap_or(usize::MAX);
        if kept < self.cap {
            self.data.extend_from_slice(vector);
            return;
        }
        let slot = self.below(self.seen);
        let Ok(slot) = usize::try_from(slot) else {
            return;
        };
        if slot < self.cap
            && let Some(at) = slot.checked_mul(self.dimension)
            && let Some(dest) = self.data.get_mut(at..at.saturating_add(self.dimension))
        {
            dest.copy_from_slice(vector);
        }
    }

    /// Vectors offered so far, kept or not.
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// The kept vectors, end to end.
    pub fn into_sample(self) -> Vec<f32> {
        self.data
    }
}

/// Whether `doc` survives `live`, the segment's deletions (`None`: none).
fn is_live(live: Option<&lucene_util::fixed_bit_set::FixedBitSet>, doc: i32) -> bool {
    live.is_none_or(|bits| usize::try_from(doc).is_ok_and(|d| bits.get(d)))
}

/// Offers every live vector of `field` in `shard` to `reservoir`.
///
/// # Errors
///
/// A field whose dimension is not the reservoir's, or vector files that do
/// not decode.
pub fn sample_shard<S: oss_blob::BlobStore + 'static>(
    shard: &OpenShard<S>,
    field: &str,
    reservoir: &mut Reservoir,
) -> Result<(), Failure> {
    let engine = |e: &dyn std::fmt::Display| {
        // ALLOC: error path; one message.
        Failure::plain("illegal_state_exception", e.to_string())
    };
    // ALLOC: one vector's floats, the field's dimension.
    let mut scratch = vec![0f32; reservoir.dimension];
    for segment in shard.reader.segment_readers() {
        let Some(info) = segment
            .field_infos()
            .field_by_name(field)
            .filter(|f| f.vector_dimension > 0)
        else {
            continue;
        };
        if usize::try_from(info.vector_dimension).ok() != Some(reservoir.dimension) {
            // ALLOC: error path; one message.
            return Err(Failure::plain(
                "illegal_argument_exception",
                format!(
                    "Field \"{field}\" has dimension {}, which is different from dimension specified in the training request: {}",
                    info.vector_dimension, reservoir.dimension
                ),
            ));
        }
        let Some((flat, _)) = segment
            .knn_vectors_for(info.number)
            .map_err(|e| engine(&e))?
        else {
            continue;
        };
        let values = flat
            .float_vector_values(info.number)
            .map_err(|e| engine(&e))?;
        let live = segment.live_docs();
        for ord in 0..values.size() {
            if live.is_some() {
                let doc = values.ord_to_doc(ord).map_err(|e| engine(&e))?;
                if !is_live(live, doc) {
                    continue;
                }
            }
            values
                .vector_into(ord, &mut scratch)
                .map_err(|e| engine(&e))?;
            reservoir.offer(&scratch);
        }
    }
    Ok(())
}

#[cfg(test)]
mod live_tests {
    use lucene_util::fixed_bit_set::FixedBitSet;

    use super::is_live;

    #[test]
    fn a_deleted_document_is_not_live_and_no_deletions_keep_all() {
        let mut bits = FixedBitSet::new(4);
        bits.set(1);
        bits.set(3);
        assert!(is_live(Some(&bits), 1));
        assert!(!is_live(Some(&bits), 0));
        assert!(!is_live(Some(&bits), 2));
        assert!(!is_live(Some(&bits), -1));
        assert!(is_live(None, 7));
    }
}
