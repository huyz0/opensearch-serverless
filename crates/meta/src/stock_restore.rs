//! Restoring one index of a stock OpenSearch snapshot: the descriptor from
//! the stock index's metadata, then each shard's files copied from the
//! source repository, whole, into the new index's first-term container.
//!
//! A file's chunks are read in ranges of one buffer and written as one blob
//! of the restored index, so memory is two buffers whatever its length. The
//! Lucene footer CRC is checked as the bytes pass, and the length against
//! the file list; a file that fails fails the restore and the caller deletes
//! the index. No LIST: the shard snapshot names every chunk.

use oss_blob::{BlobError, BlobResult, BlobStore, ByteRange, ChunkSource, SwapOutcome};
use oss_format::{
    FileLocation, FormatError, ManifestEntry, RestoreIndex, ShardRef, StockFile, StockFileCheck,
    encode_manifest, is_blob_name,
};

use crate::descriptors::DescriptorStore;
use crate::snapshot_restore::{CopyBuffers, FIRST_TERM_DIR, RestoreError, SnapshotRestorer};

/// The repository files are read from: a store the node only reads, and
/// the key prefix of the repository in it (empty, or ending in `/`).
pub struct StockSource<'a> {
    /// The source store.
    pub store: &'a dyn BlobStore,
    /// The repository's prefix in it.
    pub prefix: &'a str,
}

/// One shard of a stock index to copy.
pub struct StockShardCopy<'a> {
    /// The stock index's id, the directory under `indices/`.
    pub index_id: &'a str,
    /// The shard number.
    pub shard: u32,
    /// The shard snapshot's files.
    pub files: &'a [StockFile],
}

fn refused(why: &'static str) -> RestoreError {
    RestoreError::Format(FormatError::Corrupt {
        what: "stock repository",
        reason: why,
    })
}

impl SnapshotRestorer {
    /// Creates `target` with `uuid` from `source`, the native form of the
    /// stock index (its `shards` empty), then copies each of `shards` from
    /// `from` and lands each shard's manifest.
    ///
    /// Cost: a descriptor create, then for each file one write and, for each
    /// chunk, one ranged read for each buffer of it (none for a virtual
    /// file), and one manifest create for each shard. No LIST.
    ///
    /// # Errors
    ///
    /// [`RestoreError::Format`] for a name that is not a blob name or a file
    /// whose bytes do not match its listing; [`RestoreError::Blob`] when
    /// a chunk is missing or short; otherwise the failing step's error.
    #[allow(clippy::too_many_arguments)]
    pub fn restore_stock_index(
        &self,
        descriptors: &DescriptorStore,
        from: &StockSource<'_>,
        source: &RestoreIndex,
        shards: &[StockShardCopy<'_>],
        target: &str,
        uuid: &str,
        now_millis: i64,
        record_buf: &mut [u8],
        buffers: &mut CopyBuffers<'_>,
    ) -> Result<(), RestoreError> {
        self.restore_with(
            descriptors,
            source,
            target,
            uuid,
            now_millis,
            record_buf,
            |me| {
                for shard in shards {
                    me.restore_stock_shard(from, shard, target, uuid, buffers)?;
                }
                Ok(())
            },
        )
    }

    fn restore_stock_shard(
        &self,
        from: &StockSource<'_>,
        shard: &StockShardCopy<'_>,
        target: &str,
        uuid: &str,
        buffers: &mut CopyBuffers<'_>,
    ) -> Result<(), RestoreError> {
        if !is_blob_name(shard.index_id) {
            return Err(refused("an index id that is not a blob name"));
        }
        let to = ShardRef {
            index: target,
            uuid,
            shard: shard.shard,
        };
        let whole = FileLocation::Whole {
            term_dir: FIRST_TERM_DIR,
        };
        // ALLOC: one entry a file, at most the shard snapshot's file bound.
        let mut entries = Vec::with_capacity(shard.files.len());
        for file in shard.files {
            if !is_blob_name(&file.physical_name) || !is_blob_name(&file.name) {
                return Err(refused("a file name that is not a blob name"));
            }
            let (dest, _) = to.blob_key(&self.base, &file.physical_name, &whole)?;
            let mut body = StockFileSource::new(from, shard, file, &mut *buffers.read)?;
            self.store.put(&dest, &mut body, &mut *buffers.write)?;
            entries.push(ManifestEntry {
                name: &file.physical_name,
                location: whole.encode(),
                length: file.length,
            });
        }
        let manifest = encode_manifest(1, None, &entries, -1, -1)?;
        match self
            .store
            .create_register(&to.manifest_key(&self.base)?, &manifest)?
        {
            SwapOutcome::Applied(_) => Ok(()),
            SwapOutcome::Conflict(_) => Err(RestoreError::Conflict { shard: shard.shard }),
        }
    }
}

/// One stock file as a PUT's body: its chunks one after the other, or its
/// inline bytes, each byte fed to the check.
struct StockFileSource<'a> {
    from: &'a StockSource<'a>,
    dir: String,
    file: &'a StockFile,
    check: StockFileCheck,
    part: u64,
    in_part: u64,
    done: u64,
    scratch: &'a mut [u8],
}

impl<'a> StockFileSource<'a> {
    fn new(
        from: &'a StockSource<'a>,
        shard: &StockShardCopy<'_>,
        file: &'a StockFile,
        scratch: &'a mut [u8],
    ) -> Result<Self, RestoreError> {
        let check = StockFileCheck::new(file.length, &file.checksum)?;
        if file.is_virtual() {
            let inline = file.inline.as_deref().map_or(0, <[u8]>::len);
            if u64::try_from(inline).ok() != Some(file.length) {
                return Err(refused("a virtual file whose bytes are not its length"));
            }
        }
        // ALLOC: one key prefix a shard.
        let dir = format!("{}indices/{}/{}/", from.prefix, shard.index_id, shard.shard);
        Ok(Self {
            from,
            dir,
            file,
            check,
            part: 0,
            in_part: 0,
            done: 0,
            scratch,
        })
    }

    /// The bytes of chunk `part`: all of them but the last chunk's shorter.
    fn part_len(&self, part: u64) -> u64 {
        match self.file.part_size {
            Some(size) if self.file.parts() > 1 => self
                .file
                .length
                .saturating_sub(part.saturating_mul(size))
                .min(size),
            _ => self.file.length,
        }
    }
}

fn stream(e: FormatError) -> BlobError {
    // The check's words name no key; the store error carries the reason.
    let _ = e;
    BlobError::Stream("a stock file does not match its listing")
}

impl ChunkSource for StockFileSource<'_> {
    fn len(&self) -> u64 {
        self.file.length
    }

    fn fill(&mut self, buf: &mut [u8]) -> BlobResult<usize> {
        let left = self.file.length.saturating_sub(self.done);
        let want = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        if want == 0 {
            return Ok(0);
        }
        let got = if self.file.is_virtual() {
            let start = usize::try_from(self.done).unwrap_or(usize::MAX);
            let bytes = self
                .file
                .inline
                .as_deref()
                .and_then(|b| b.get(start..start.saturating_add(want)))
                .ok_or(BlobError::Stream("a virtual file is shorter than listed"))?;
            buf.get_mut(..want)
                .ok_or(BlobError::Stream("a fill past its buffer"))?
                .copy_from_slice(bytes);
            want
        } else {
            if self.in_part >= self.part_len(self.part) {
                self.part = self.part.saturating_add(1);
                self.in_part = 0;
            }
            let room = self.part_len(self.part).saturating_sub(self.in_part);
            let want = want.min(usize::try_from(room).unwrap_or(usize::MAX));
            // ALLOC: one key a chunk read, at most 200 bytes.
            let key = [self.dir.as_str(), &self.file.part_name(self.part)].concat();
            let range = ByteRange::From {
                start: self.in_part,
                len: u64::try_from(want).unwrap_or(u64::MAX),
            };
            let mut filled = 0usize;
            let mut sink = |chunk: &[u8]| -> BlobResult<()> {
                let end = filled.saturating_add(chunk.len());
                buf.get_mut(filled..end)
                    .ok_or(BlobError::Stream("a read returned more than asked"))?
                    .copy_from_slice(chunk);
                filled = end;
                Ok(())
            };
            self.from
                .store
                .get(&key, range, &mut *self.scratch, &mut sink)?;
            if filled != want {
                return Err(BlobError::Stream("a stock chunk is shorter than listed"));
            }
            self.in_part = self
                .in_part
                .saturating_add(u64::try_from(want).unwrap_or(0));
            want
        };
        let bytes = buf.get(..got).unwrap_or(&[]);
        self.check.update(bytes).map_err(stream)?;
        self.done = self.done.saturating_add(u64::try_from(got).unwrap_or(0));
        if self.done == self.file.length {
            self.check.finish().map_err(stream)?;
        }
        Ok(got)
    }

    fn rewind(&mut self) -> BlobResult<()> {
        self.part = 0;
        self.in_part = 0;
        self.done = 0;
        self.check.reset();
        Ok(())
    }
}
