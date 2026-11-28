//! The blob of a trained k-NN model: its coarse centroids and, for a product
//! quantizer, its codebooks (docs/adr/0087).
//!
//! The layout is this node's own, versioned and checksummed, not Faiss's:
//! a 24-byte header, the `nlist x dimension` centroids as little-endian
//! `f32`, then `m x 256 x (dimension / m)` codebook floats, then the CRC-32
//! of everything before it.
//!
//! ```text
//! magic "OKNM" | version u32 | space u8 | encoder u8 | pad u16 |
//! dimension u32 | nlist u32 | m u32 (0 for flat)
//! ```

use crate::error::FormatError;

const WHAT: &str = "k-NN model blob";
const MAGIC: &[u8; 4] = b"OKNM";
const VERSION: u32 = 1;
const HEADER: usize = 24;
const CRC: usize = 4;
/// Entries a product quantizer's sub-codebook holds: 8-bit codes.
pub const PQ_ENTRIES: usize = 256;
/// The largest blob this reader accepts (ADR 0087's 64 MiB).
pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;

/// The space a model's vectors are compared in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSpace {
    /// Euclidean distance.
    L2,
    /// Inner product.
    InnerProduct,
}

/// What encodes a vector within its cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelEncoder {
    /// The vector itself.
    Flat,
    /// A product quantizer of `m` sub-vectors with 8-bit codes.
    Pq {
        /// Sub-vectors a vector is split into; divides the dimension.
        m: u32,
    },
}

/// A model's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelHeader {
    /// The space.
    pub space: ModelSpace,
    /// Floats in a vector.
    pub dimension: u32,
    /// Coarse clusters.
    pub nlist: u32,
    /// The encoder.
    pub encoder: ModelEncoder,
}

fn corrupt(reason: &'static str) -> FormatError {
    FormatError::Corrupt { what: WHAT, reason }
}

impl ModelHeader {
    /// Floats in the centroids.
    fn centroid_floats(&self) -> Option<usize> {
        usize::try_from(self.nlist)
            .ok()?
            .checked_mul(usize::try_from(self.dimension).ok()?)
    }

    /// Floats in the codebooks.
    fn codebook_floats(&self) -> Option<usize> {
        match self.encoder {
            ModelEncoder::Flat => Some(0),
            ModelEncoder::Pq { m } => {
                let m = usize::try_from(m).ok()?;
                let dimension = usize::try_from(self.dimension).ok()?;
                let sub = dimension.checked_div(m)?;
                m.checked_mul(PQ_ENTRIES)?.checked_mul(sub)
            }
        }
    }

    /// The encoded size of a model of this shape, or `None` past
    /// [`MAX_BLOB_BYTES`] or for a shape that cannot be.
    #[must_use]
    pub fn encoded_len(&self) -> Option<usize> {
        if self.dimension == 0 || self.nlist == 0 {
            return None;
        }
        if let ModelEncoder::Pq { m } = self.encoder
            && (m == 0 || !self.dimension.is_multiple_of(m))
        {
            return None;
        }
        let floats = self
            .centroid_floats()?
            .checked_add(self.codebook_floats()?)?;
        let total = floats
            .checked_mul(4)?
            .checked_add(HEADER)?
            .checked_add(CRC)?;
        (total <= MAX_BLOB_BYTES).then_some(total)
    }
}

/// Encodes a model.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a shape that cannot be, or floats that do
/// not fill it; [`FormatError::TooLarge`] past [`MAX_BLOB_BYTES`].
pub fn encode(
    header: &ModelHeader,
    centroids: &[f32],
    codebooks: &[f32],
) -> Result<Vec<u8>, FormatError> {
    let total = header.encoded_len().ok_or(FormatError::TooLarge {
        what: WHAT,
        limit: MAX_BLOB_BYTES as u64,
        actual: 0,
    })?;
    if Some(centroids.len()) != header.centroid_floats()
        || Some(codebooks.len()) != header.codebook_floats()
    {
        return Err(corrupt("the floats do not fill the model's shape"));
    }
    // ALLOC: one blob, `encoded_len` bytes, at most 64 MiB; the caller reserved it.
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let (encoder, m) = match header.encoder {
        ModelEncoder::Flat => (0u8, 0u32),
        ModelEncoder::Pq { m } => (1u8, m),
    };
    out.push(match header.space {
        ModelSpace::L2 => 0,
        ModelSpace::InnerProduct => 1,
    });
    out.push(encoder);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&header.dimension.to_le_bytes());
    out.extend_from_slice(&header.nlist.to_le_bytes());
    out.extend_from_slice(&m.to_le_bytes());
    for v in centroids.iter().chain(codebooks) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    Ok(out)
}

/// A parsed model: its shape and the bytes of its floats, borrowed.
#[derive(Clone, Copy, Debug)]
pub struct ModelView<'a> {
    header: ModelHeader,
    centroids: &'a [u8],
    codebooks: &'a [u8],
    crc: u32,
}

fn floats(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl<'a> ModelView<'a> {
    /// The model's shape.
    #[must_use]
    pub fn header(&self) -> &ModelHeader {
        &self.header
    }

    /// The CRC-32 the blob carries, which a segment names to tie its codes to this model.
    #[must_use]
    pub fn crc(&self) -> u32 {
        self.crc
    }

    /// Centroid `i`'s floats, or `None` past `nlist`.
    pub fn centroid(&self, i: usize) -> Option<impl Iterator<Item = f32> + 'a> {
        let width = usize::try_from(self.header.dimension)
            .ok()?
            .checked_mul(4)?;
        let at = i.checked_mul(width)?;
        Some(floats(self.centroids.get(at..at.checked_add(width)?)?))
    }

    /// Every codebook float, sub-quantizer major.
    pub fn codebook_floats(&self) -> impl Iterator<Item = f32> + 'a {
        floats(self.codebooks)
    }
}

/// Reads a model.
///
/// # Errors
///
/// [`FormatError::Truncated`], [`FormatError::Unsupported`] for another
/// version, [`FormatError::Corrupt`] for a bad magic, shape or checksum,
/// [`FormatError::TooLarge`] past [`MAX_BLOB_BYTES`].
pub fn parse(bytes: &[u8]) -> Result<ModelView<'_>, FormatError> {
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(FormatError::TooLarge {
            what: WHAT,
            limit: MAX_BLOB_BYTES as u64,
            actual: bytes.len() as u64,
        });
    }
    let have = bytes.len() as u64;
    let head = bytes.get(..HEADER).ok_or(FormatError::Truncated {
        what: WHAT,
        needed: HEADER as u64,
        have,
    })?;
    if head.get(..4) != Some(MAGIC.as_slice()) {
        return Err(corrupt("the magic is wrong"));
    }
    let word = |at: usize| -> Option<u32> {
        let b = head.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let version = word(4).ok_or(corrupt("the header is short"))?;
    if version != VERSION {
        return Err(FormatError::Unsupported {
            what: WHAT,
            version: u64::from(version),
        });
    }
    let space = match head.get(8) {
        Some(0) => ModelSpace::L2,
        Some(1) => ModelSpace::InnerProduct,
        _ => return Err(corrupt("the space is unknown")),
    };
    let dimension = word(12).ok_or(corrupt("the header is short"))?;
    let nlist = word(16).ok_or(corrupt("the header is short"))?;
    let m = word(20).ok_or(corrupt("the header is short"))?;
    let encoder = match head.get(9) {
        Some(0) if m == 0 => ModelEncoder::Flat,
        Some(1) => ModelEncoder::Pq { m },
        _ => return Err(corrupt("the encoder is unknown")),
    };
    let header = ModelHeader {
        space,
        dimension,
        nlist,
        encoder,
    };
    let total = header
        .encoded_len()
        .ok_or(corrupt("the shape is impossible or too large"))?;
    if bytes.len() < total {
        return Err(FormatError::Truncated {
            what: WHAT,
            needed: total as u64,
            have,
        });
    }
    if bytes.len() > total {
        return Err(corrupt("bytes follow the checksum"));
    }
    let body_end = total.saturating_sub(CRC);
    let body = bytes.get(..body_end).ok_or(corrupt("the body is short"))?;
    let tail = bytes
        .get(body_end..)
        .ok_or(corrupt("the checksum is short"))?;
    let crc = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
    if crc32fast::hash(body) != crc {
        return Err(corrupt("the checksum does not match"));
    }
    let centroid_bytes = header
        .centroid_floats()
        .and_then(|f| f.checked_mul(4))
        .and_then(|b| b.checked_add(HEADER))
        .ok_or(corrupt("the shape is impossible"))?;
    Ok(ModelView {
        header,
        centroids: body
            .get(HEADER..centroid_bytes)
            .ok_or(corrupt("the centroids are short"))?,
        codebooks: body
            .get(centroid_bytes..)
            .ok_or(corrupt("the codebooks are short"))?,
        crc,
    })
}

#[cfg(test)]
#[path = "knn_model_tests.rs"]
mod tests;
