//! Trains a k-NN model from a vector sample: coarse centroids by Faiss's
//! `Clustering` as `lucene-faiss` ports it, and for a `pq` encoder the
//! product-quantizer codebooks on residuals (the vector minus its nearest
//! centroid), as `IndexIVFPQ` does. The result is the model blob of
//! [ADR 0087](../../../docs/adr/0087-knn-ivf-and-pq-search-from-a-shared-model.md).

use lucene_faiss::format::Flat;
use lucene_faiss::{Metric, kmeans};
use lucene_vector_quant::pq::PqFactory;
use lucene_vector_quant::{QuantizerFactory, Similarity};
use oss_format::{ModelEncoder, ModelHeader, ModelSpace, encode_model};

/// Fixes the sample order the clustering draws, so a run is repeatable.
const SEED: u64 = 0x4b4e_4e5f_5452_4149;

/// What a model is trained for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TrainSpec {
    pub dimension: usize,
    pub nlist: usize,
    pub space: ModelSpace,
    /// Sub-quantizers of a `pq` encoder; `None` for `flat`.
    pub pq_m: Option<usize>,
    /// Faiss's points per centroid, 256 unless the pool cuts it.
    pub points_per_centroid: usize,
}

/// A training that cannot produce a model, with the message the model's
/// `error` field carries.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TrainFailure(pub String);

/// Trains a model on `sample`, vectors of `spec.dimension` floats laid end
/// to end, and returns its blob.
pub(crate) fn train(sample: &[f32], spec: &TrainSpec) -> Result<Vec<u8>, TrainFailure> {
    let metric = match spec.space {
        ModelSpace::L2 => Metric::L2,
        ModelSpace::InnerProduct => Metric::InnerProduct,
    };
    let coarse = kmeans::cluster(
        sample,
        spec.dimension,
        spec.nlist,
        metric,
        spec.points_per_centroid,
        SEED,
    )
    .map_err(failure)?;
    let (encoder, books) = match spec.pq_m {
        None => (ModelEncoder::Flat, Vec::new()),
        Some(m) => (
            ModelEncoder::Pq {
                m: u32::try_from(m).map_err(|_| TrainFailure("m is too large".into()))?,
            },
            product_codebooks(sample, &coarse, spec, m)?,
        ),
    };
    let header = ModelHeader {
        space: spec.space,
        dimension: u32::try_from(spec.dimension)
            .map_err(|_| TrainFailure("dimension is too large".into()))?,
        nlist: u32::try_from(spec.nlist).map_err(|_| TrainFailure("nlist is too large".into()))?,
        encoder,
    };
    encode_model(&header, &coarse.data, &books).map_err(|e| TrainFailure(e.to_string()))
}

fn failure(e: lucene_faiss::Error) -> TrainFailure {
    let text = e.to_string();
    // ALLOC: error path; one message.
    TrainFailure(
        text.strip_prefix("invalid argument: ")
            .unwrap_or(&text)
            .to_owned(),
    )
}

/// The codebooks of `m` sub-quantizers trained on each vector's residual
/// against its nearest coarse centroid.
fn product_codebooks(
    sample: &[f32],
    coarse: &Flat,
    spec: &TrainSpec,
    m: usize,
) -> Result<Vec<f32>, TrainFailure> {
    let d = spec.dimension;
    // ALLOC: one residual per sample vector, the sample's size (ADR 0087's bound counts it).
    let mut residuals = Vec::with_capacity(sample.len());
    for v in sample.chunks_exact(d) {
        let c = nearest(v, coarse, spec.space);
        residuals.extend(v.iter().zip(c).map(|(a, b)| a - b));
    }
    let similarity = match spec.space {
        ModelSpace::L2 => Similarity::Euclidean,
        ModelSpace::InnerProduct => Similarity::DotProduct,
    };
    let quantizer = PqFactory::new(m)
        .train(d, similarity, &residuals)
        .map_err(|e| TrainFailure(plain(&e.to_string())))?;
    let state = quantizer.state();
    // The state is `m` as a u32, then the codebooks.
    Ok(state
        .get(4..)
        .unwrap_or_default()
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// The centroid `v` is nearest to: least squared distance, or most inner product.
fn nearest<'c>(v: &[f32], coarse: &'c Flat, space: ModelSpace) -> &'c [f32] {
    let d = coarse.d;
    let score = |c: &[f32]| -> f32 {
        match space {
            ModelSpace::L2 => c.iter().zip(v).map(|(a, b)| (a - b) * (a - b)).sum(),
            ModelSpace::InnerProduct => -c.iter().zip(v).map(|(a, b)| a * b).sum::<f32>(),
        }
    };
    coarse
        .data
        .chunks_exact(d)
        .min_by(|a, b| score(a).total_cmp(&score(b)))
        .unwrap_or(&[])
}

/// A library message without its error prefix.
fn plain(text: &str) -> String {
    // ALLOC: error path; one message.
    text.rsplit_once(": ")
        .map_or(text, |(_, rest)| rest)
        .to_owned()
}

#[cfg(test)]
#[path = "knn_trainer_tests.rs"]
mod tests;
