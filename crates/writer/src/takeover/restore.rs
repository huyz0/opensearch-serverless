//! Restoring a shard's published commit into a writer's directory.

use std::collections::BTreeMap;
use std::fmt;

use lucene_store::{DataOutput, Directory, FsIndexOutput};
use oss_blob::{BlobError, BlobStore, ByteRange, ChunkSink};
use oss_format::{CommitManifest, FormatError, ShardRef};

/// What a register read first has room for: well over a thousand files.
const READ_FIRST: usize = 256 << 10;

/// The most a manifest read keeps: 16,384 files at well under 200 bytes.
const READ_MOST: usize = 4 << 20;

/// The commit a restore found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Restored {
    /// The commit's primary term.
    pub term: u64,
    /// The register's generation.
    pub generation: u64,
}

/// Why a commit was not restored; the directory is left for the caller to
/// drop.
#[derive(Debug)]
pub enum RestoreError {
    /// The store refused or failed a request.
    Blob(BlobError),
    /// The manifest does not parse or does not resolve.
    Format(FormatError),
    /// The directory refused a file.
    Directory(String),
    /// A blob ended before the file it holds (the named one) did.
    Short(String),
    /// A manifest too large to read.
    TooLarge,
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blob(e) => write!(f, "reading the commit: {e}"),
            Self::Format(e) => write!(f, "the commit's manifest: {e}"),
            Self::Directory(e) => write!(f, "writing a restored file: {e}"),
            Self::Short(name) => write!(f, "the blob of [{name}] ends before the file does"),
            Self::TooLarge => f.write_str("a manifest too large to read"),
        }
    }
}

impl std::error::Error for RestoreError {}

impl From<BlobError> for RestoreError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

impl From<FormatError> for RestoreError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

/// One file of a blob: where it starts and how long it is, if known.
struct Member {
    name: String,
    offset: u64,
    length: Option<u64>,
}

/// Copies the files of the commit `shard`'s register names into `dir`, one
/// ranged GET a blob through `buf`. `None` when the register names no
/// commit.
///
/// # Errors
///
/// A [`RestoreError`] when a request fails, the manifest cannot be read, the
/// directory refuses a file or a blob is shorter than its files.
pub fn restore(
    store: &dyn BlobStore,
    base: &str,
    shard: &ShardRef<'_>,
    dir: &dyn Directory,
    buf: &mut [u8],
) -> Result<Option<Restored>, RestoreError> {
    let key = shard.manifest_key(base)?;
    let Some((value, generation)) = read_manifest(store, &key)? else {
        return Ok(None);
    };
    let manifest = CommitManifest::parse(&value)?;
    // ALLOC: one entry a blob of the commit, one member a file.
    let mut blobs: BTreeMap<String, Vec<Member>> = BTreeMap::new();
    for extent in shard.resolve(base, &manifest)? {
        blobs.entry(extent.key).or_default().push(Member {
            name: extent.name.into_owned(),
            offset: extent.offset,
            length: extent.length,
        });
    }
    for (blob, mut members) in blobs {
        members.sort_by_key(|m| m.offset);
        spread(store, &blob, &members, dir, buf)?;
    }
    Ok(Some(Restored {
        term: manifest.term,
        generation,
    }))
}

/// The register's value and generation; `None` when there is none.
fn read_manifest(store: &dyn BlobStore, key: &str) -> Result<Option<(Vec<u8>, u64)>, RestoreError> {
    for size in [READ_FIRST, READ_MOST] {
        // ALLOC: one buffer a restore's register read, at most manifest-sized.
        let mut buf = vec![0u8; size];
        match store.read_register(key, &mut buf) {
            Ok(Some(read)) => {
                buf.truncate(read.len);
                return Ok(Some((buf, read.generation)));
            }
            Ok(None) => return Ok(None),
            // A larger manifest than this buffer: once more with room for the most.
            Err(BlobError::InvalidArgument(_)) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(RestoreError::TooLarge)
}

/// Streams the span of `blob` that `members` cover into one new file each.
fn spread(
    store: &dyn BlobStore,
    blob: &str,
    members: &[Member],
    dir: &dyn Directory,
    buf: &mut [u8],
) -> Result<(), RestoreError> {
    let first = members.first().map_or(0, |m| m.offset);
    let whole = members.len() == 1 && first == 0 && members.iter().all(|m| m.length.is_none());
    let end = members.iter().try_fold(first, |end, m| {
        m.length.map(|l| end.max(m.offset.saturating_add(l)))
    });
    let range = match (whole, end) {
        (false, Some(end)) if end > first => ByteRange::From {
            start: first,
            len: end.saturating_sub(first),
        },
        (false, Some(_)) => return empty(members, dir),
        _ => ByteRange::Full,
    };
    let mut sink = Spread {
        dir,
        members,
        at: first,
        next: 0,
        out: None,
        failed: None,
    };
    let outcome = store.get(blob, range, buf, &mut sink);
    if let Some(e) = sink.failed.take() {
        return Err(RestoreError::Directory(e));
    }
    match outcome {
        Ok(_) => sink.finish(),
        // A blob shorter than where its files start.
        Err(BlobError::InvalidRange) => Err(RestoreError::Short(
            // ALLOC: error path; one name.
            members.first().map(|m| m.name.clone()).unwrap_or_default(),
        )),
        Err(e) => Err(e.into()),
    }
}

/// Files of no bytes: created, with nothing to read.
fn empty(members: &[Member], dir: &dyn Directory) -> Result<(), RestoreError> {
    for m in members {
        dir.create_output(&m.name)
            .and_then(FsIndexOutput::close)
            // ALLOC: error path; one message.
            .map_err(|e| RestoreError::Directory(e.to_string()))?;
    }
    Ok(())
}

/// A GET's sink that cuts the stream into the members it holds.
struct Spread<'a> {
    dir: &'a dyn Directory,
    members: &'a [Member],
    /// The blob offset of the next byte.
    at: u64,
    /// The member being written, or the next to start.
    next: usize,
    out: Option<FsIndexOutput>,
    failed: Option<String>,
}

impl Spread<'_> {
    /// Ends the last member, and checks the stream held every file.
    fn finish(mut self) -> Result<(), RestoreError> {
        let Some(m) = self.members.get(self.next) else {
            return Ok(());
        };
        // A whole blob of unknown length ends with the stream.
        if m.length.is_none() && self.next.saturating_add(1) == self.members.len() {
            if let Some(out) = self.out.take() {
                out.close()
                    // ALLOC: error path; one message.
                    .map_err(|e| RestoreError::Directory(e.to_string()))?;
            } else {
                empty(self.members.get(self.next..).unwrap_or_default(), self.dir)?;
            }
            return Ok(());
        }
        // ALLOC: error path; one name.
        Err(RestoreError::Short(m.name.clone()))
    }
}

impl ChunkSink for Spread<'_> {
    fn accept(&mut self, chunk: &[u8]) -> oss_blob::BlobResult<()> {
        let mut rest = chunk;
        while !rest.is_empty() {
            let Some(m) = self.members.get(self.next) else {
                // Bytes past the last file are not part of the commit.
                return Ok(());
            };
            if self.out.is_none() && self.at < m.offset {
                let skip = usize::try_from(m.offset.saturating_sub(self.at))
                    .unwrap_or(usize::MAX)
                    .min(rest.len());
                rest = rest.get(skip..).unwrap_or_default();
                self.at = self.at.saturating_add(u64::try_from(skip).unwrap_or(0));
                continue;
            }
            if self.out.is_none() {
                match self.dir.create_output(&m.name) {
                    Ok(out) => self.out = Some(out),
                    Err(e) => {
                        // ALLOC: error path; one message.
                        self.failed = Some(e.to_string());
                        return Err(BlobError::Stream("the directory refused a file"));
                    }
                }
            }
            let end = m.length.map(|l| m.offset.saturating_add(l));
            let room = end.map_or(usize::MAX, |end| {
                usize::try_from(end.saturating_sub(self.at)).unwrap_or(usize::MAX)
            });
            let take = room.min(rest.len());
            let (head, tail) = rest.split_at(take);
            if let Some(out) = self.out.as_mut() {
                out.write_bytes(head);
            }
            self.at = self.at.saturating_add(u64::try_from(take).unwrap_or(0));
            rest = tail;
            if end.is_some_and(|end| self.at >= end) {
                if let Some(out) = self.out.take()
                    && let Err(e) = out.close()
                {
                    // ALLOC: error path; one message.
                    self.failed = Some(e.to_string());
                    return Err(BlobError::Stream("the directory refused a file"));
                }
                self.next = self.next.saturating_add(1);
            }
        }
        Ok(())
    }
}
