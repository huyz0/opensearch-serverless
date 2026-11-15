//! The Java serverless shell's blob formats.
//!
//! Segment blobs (one immutable blob per segment, with a header listing each
//! file's offset), node packs, and commit descriptors and manifests, ported
//! from the shell so Rust and Java nodes read and write the same bucket.
//! Pure parsing over bytes; the store lives in `oss-blob`.
