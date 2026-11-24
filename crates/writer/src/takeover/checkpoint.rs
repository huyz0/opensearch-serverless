//! The local checkpoint a shard's published commit records, read from the
//! commit's `segments_N` alone.

use lucene_index::segment_infos::parse;
use oss_blob::{BlobError, BlobStore, ByteRange, ChunkSink};
use oss_format::{CommitManifest, ShardRef};

use super::RestoreError;
use super::restore::read_manifest;
use crate::index::LOCAL_CHECKPOINT_KEY;

/// The most a `segments_N` may be: it holds one entry a segment of the
/// commit, well under this.
const MAX_SEGMENTS_FILE: usize = 4 << 20;

/// Collects a ranged GET into one buffer, refusing a stream past `MAX_SEGMENTS_FILE`.
struct Collect(Vec<u8>);

impl ChunkSink for Collect {
    fn accept(&mut self, chunk: &[u8]) -> oss_blob::BlobResult<()> {
        if self.0.len().saturating_add(chunk.len()) > MAX_SEGMENTS_FILE {
            return Err(BlobError::InvalidArgument(
                "a segments file larger than a commit's can be",
            ));
        }
        self.0.extend_from_slice(chunk);
        Ok(())
    }
}

/// The generation a `segments_N` file name carries (`N` is base 36).
fn generation(name: &str) -> Option<i64> {
    let digits = name.strip_prefix("segments_")?;
    i64::from_str_radix(digits, 36).ok()
}

/// The local checkpoint recorded in the commit `shard`'s register names:
/// `None` when it names no commit, `Some(None)` when the commit records no
/// checkpoint. One register read and one ranged GET of `segments_N`.
///
/// # Errors
///
/// A [`RestoreError`] when a request fails or the manifest or `segments_N`
/// cannot be read.
pub fn commit_checkpoint(
    store: &dyn BlobStore,
    base: &str,
    shard: &ShardRef<'_>,
) -> Result<Option<Option<i64>>, RestoreError> {
    let key = shard.manifest_key(base)?;
    let Some((value, _)) = read_manifest(store, &key)? else {
        return Ok(None);
    };
    let manifest = CommitManifest::parse(&value)?;
    let newest = shard
        .resolve(base, &manifest)?
        .into_iter()
        .filter_map(|extent| generation(&extent.name).map(|g| (g, extent)))
        .max_by_key(|(g, _)| *g);
    let Some((generation, extent)) = newest else {
        return Ok(Some(None));
    };
    let range = match extent.length {
        Some(len) => ByteRange::From {
            start: extent.offset,
            len,
        },
        None => ByteRange::Full,
    };
    // ALLOC: the segments file, at most MAX_SEGMENTS_FILE, once a commit judged.
    let mut sink = Collect(Vec::new());
    // ALLOC: the ranged GET's chunk buffer, once a commit judged.
    let mut buf = vec![0u8; 64 << 10];
    store.get(&extent.key, range, &mut buf, &mut sink)?;
    let infos = parse(&sink.0, generation).map_err(|e| RestoreError::Directory(e.to_string()))?;
    Ok(Some(recorded(&infos.user_data)))
}

/// The local checkpoint among a commit's user data, if it records one.
fn recorded(user_data: &[(String, String)]) -> Option<i64> {
    user_data
        .iter()
        .find(|(k, _)| k == LOCAL_CHECKPOINT_KEY)
        .and_then(|(_, v)| v.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(k: &str, v: &str) -> (String, String) {
        (k.to_owned(), v.to_owned())
    }

    #[test]
    fn a_segments_file_name_carries_its_generation_in_base_36() {
        assert_eq!(generation("segments_1"), Some(1));
        assert_eq!(generation("segments_a"), Some(10));
        assert_eq!(generation("segments_10"), Some(36));
        assert_eq!(generation("segments_"), None);
        assert_eq!(generation("segments_!"), None);
        assert_eq!(generation("_1.si"), None);
    }

    #[test]
    fn the_checkpoint_is_the_value_under_its_own_key_among_others() {
        let data = [
            pair("max_seq_no", "9"),
            pair("local_checkpoint", "3"),
            pair("other", "7"),
        ];
        assert_eq!(recorded(&data), Some(3));
        assert_eq!(recorded(&[pair("max_seq_no", "9")]), None);
        assert_eq!(recorded(&[pair("local_checkpoint", "x")]), None);
        assert_eq!(recorded(&[]), None);
    }

    #[test]
    fn a_stream_may_fill_the_buffer_to_the_limit_and_not_past_it() {
        let mut c = Collect(Vec::new());
        assert!(c.accept(&vec![0u8; MAX_SEGMENTS_FILE]).is_ok());
        assert!(c.accept(&[1]).is_err(), "one byte past the limit");
        let mut c = Collect(Vec::new());
        assert!(c.accept(&vec![0u8; MAX_SEGMENTS_FILE + 1]).is_err());
        let mut c = Collect(Vec::new());
        assert!(c.accept(&vec![0u8; MAX_SEGMENTS_FILE - 1]).is_ok());
        assert!(c.accept(&[1]).is_ok(), "exactly the limit");
    }
}
