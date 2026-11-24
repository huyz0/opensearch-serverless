//! The warm-copy messages: a joining reader asks a peer which cache pages it
//! holds hot, then asks for some of them.
//!
//! These actions exist only between Rust nodes; a Java node answers them
//! with an unknown-action refusal. The hot set names pages, never bytes: a
//! file is its cache key, a page is its number and stored length. The pages
//! are asked for by `(key, page)`, at most [`MAX_PAGES_PER_REQUEST`] at a
//! time, and answered from what the peer holds.

use crate::actions::tail::ShardRef;
use crate::error::WireError;
use crate::stream::{Input, Output, TaskId};

/// `internal:serverless/warm/hot_set`.
pub const HOT_SET_ACTION: &str = "internal:serverless/warm/hot_set";

/// `internal:serverless/warm/pages`.
pub const PAGES_ACTION: &str = "internal:serverless/warm/pages";

/// The most pages one request asks for.
pub const MAX_PAGES_PER_REQUEST: usize = 64;

/// The fewest bytes one shard takes: two string lengths, a shard number and a
/// file count.
const MIN_SHARD_BYTES: usize = 4;

/// The fewest bytes one file takes: a one-byte key length and a page count.
const MIN_FILE_BYTES: usize = 2;

/// The fewest bytes one page takes: a one-byte number and a one-byte length.
const MIN_PAGE_BYTES: usize = 2;

/// The fewest bytes one served page takes: key length, number, byte count.
const MIN_SERVED_BYTES: usize = 3;

/// The most entries a decoder sizes for before reading them.
const PRESIZE_LIMIT: usize = 64;

fn page_number(input: &mut Input<'_>, what: &'static str) -> Result<u64, WireError> {
    u64::try_from(input.vlong(what)?)
        .map_err(|_| WireError::malformed(what, "a page number is not negative"))
}

fn write_page_number(out: &mut Output<'_>, page: u64) -> Result<(), WireError> {
    let page = i64::try_from(page)
        .map_err(|_| WireError::too_large("a page number", i64::MAX as usize, usize::MAX))?;
    out.vlong(page)
}

/// A joiner's ask of a peer for the pages it holds hot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HotSetRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The asking node, whose shards the peer picks the pages for.
    pub joiner: String,
    /// The most shards the answer names.
    pub max_shards: i32,
    /// The most stored bytes the answer names.
    pub max_bytes: i64,
}

impl HotSetRequest {
    /// Writes the request.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.string(&self.joiner)?;
        out.vint(self.max_shards)?;
        out.vlong(self.max_bytes)
    }

    /// Reads the request.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a warm hot-set request";
        Ok(Self {
            parent: TaskId::read(input)?,
            joiner: input.string(WHAT)?,
            max_shards: input.vint(WHAT)?,
            max_bytes: input.vlong(WHAT)?,
        })
    }
}

/// One page of a file a peer holds: its number and stored length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotPage {
    /// The page number in the file.
    pub page: u64,
    /// The bytes the page holds.
    pub len: u32,
}

/// One file's hot pages, the file named by its cache key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HotFile {
    /// The file's page-cache key.
    pub key: String,
    /// Its hot pages, hottest first.
    pub pages: Vec<HotPage>,
}

/// A peer's answer: the page size it caches by and, per shard, its hot pages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HotSetResponse {
    /// The peer's page size; a joiner of another size refuses the peer.
    pub page_size: i32,
    /// The shards, each with its files.
    pub shards: Vec<(ShardRef, Vec<HotFile>)>,
}

impl HotSetResponse {
    /// Writes the answer.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.vint(self.page_size)?;
        out.count(self.shards.len())?;
        for (shard, files) in &self.shards {
            shard.write(out)?;
            out.count(files.len())?;
            for file in files {
                out.string(&file.key)?;
                out.count(file.pages.len())?;
                for hot in &file.pages {
                    write_page_number(out, hot.page)?;
                    out.vlong(i64::from(hot.len))?;
                }
            }
        }
        Ok(())
    }

    /// Reads the answer.
    ///
    /// # Errors
    ///
    /// As the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a warm hot-set answer";
        let page_size = input.vint(WHAT)?;
        let count = input.count(MIN_SHARD_BYTES, WHAT)?;
        // ALLOC: one entry per shard read, at most one per 4 bytes of the
        // frame; pre-sized to PRESIZE_LIMIT.
        let mut shards = Vec::with_capacity(count.min(PRESIZE_LIMIT));
        for _ in 0..count {
            let shard = ShardRef::read(input, WHAT)?;
            let files = input.count(MIN_FILE_BYTES, WHAT)?;
            // ALLOC: one entry per file read, at most one per 2 bytes of the
            // frame; pre-sized to PRESIZE_LIMIT.
            let mut held = Vec::with_capacity(files.min(PRESIZE_LIMIT));
            for _ in 0..files {
                let key = input.string(WHAT)?;
                let pages = input.count(MIN_PAGE_BYTES, WHAT)?;
                // ALLOC: one entry per page read, at most one per 2 bytes of
                // the frame; pre-sized to PRESIZE_LIMIT.
                let mut hot = Vec::with_capacity(pages.min(PRESIZE_LIMIT));
                for _ in 0..pages {
                    let page = page_number(input, WHAT)?;
                    let len = u32::try_from(input.vlong(WHAT)?)
                        .map_err(|_| WireError::malformed(WHAT, "a page length fits 32 bits"))?;
                    hot.push(HotPage { page, len });
                }
                held.push(HotFile { key, pages: hot });
            }
            shards.push((shard, held));
        }
        Ok(Self { page_size, shards })
    }
}

/// One page asked for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageAsk {
    /// The file's page-cache key.
    pub key: String,
    /// The page number.
    pub page: u64,
}

/// A joiner's ask for pages, at most [`MAX_PAGES_PER_REQUEST`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PagesRequest {
    /// The parent task.
    pub parent: TaskId,
    /// The pages asked for.
    pub asks: Vec<PageAsk>,
}

impl PagesRequest {
    /// Writes the request.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        self.parent.write(out)?;
        out.count(self.asks.len())?;
        for ask in &self.asks {
            out.string(&ask.key)?;
            write_page_number(out, ask.page)?;
        }
        Ok(())
    }

    /// Reads the request.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] for more than [`MAX_PAGES_PER_REQUEST`] asks,
    /// before any is read; otherwise as the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a warm pages request";
        let parent = TaskId::read(input)?;
        let count = input.vint(WHAT)?;
        let count = usize::try_from(count)
            .map_err(|_| WireError::malformed(WHAT, "a count is not negative"))?;
        if count > MAX_PAGES_PER_REQUEST {
            return Err(WireError::too_large(WHAT, MAX_PAGES_PER_REQUEST, count));
        }
        // ALLOC: one entry per ask, at most MAX_PAGES_PER_REQUEST.
        let mut asks = Vec::with_capacity(count);
        for _ in 0..count {
            asks.push(PageAsk {
                key: input.string(WHAT)?,
                page: page_number(input, WHAT)?,
            });
        }
        Ok(Self { parent, asks })
    }
}

/// One page a peer held and serves.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageBytes {
    /// The file's page-cache key.
    pub key: String,
    /// The page number.
    pub page: u64,
    /// The page as the peer holds it.
    pub bytes: Vec<u8>,
}

/// A peer's answer: the asked pages it still held, in the order asked. A page
/// it no longer holds is left out, never fetched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PagesResponse {
    /// The pages served.
    pub pages: Vec<PageBytes>,
}

impl PagesResponse {
    /// Writes the answer.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] past the writer's limit.
    pub fn write(&self, out: &mut Output<'_>) -> Result<(), WireError> {
        out.count(self.pages.len())?;
        for page in &self.pages {
            out.string(&page.key)?;
            write_page_number(out, page.page)?;
            out.bytes_reference(&page.bytes)?;
        }
        Ok(())
    }

    /// Reads the answer.
    ///
    /// # Errors
    ///
    /// [`WireError::TooLarge`] for more than [`MAX_PAGES_PER_REQUEST`] pages;
    /// otherwise as the reads it makes.
    pub fn read(input: &mut Input<'_>) -> Result<Self, WireError> {
        const WHAT: &str = "a warm pages answer";
        let count = input.count(MIN_SERVED_BYTES, WHAT)?;
        if count > MAX_PAGES_PER_REQUEST {
            return Err(WireError::too_large(WHAT, MAX_PAGES_PER_REQUEST, count));
        }
        // ALLOC: one entry per page served, at most MAX_PAGES_PER_REQUEST;
        // the bytes are the pages the joiner asked for.
        let mut pages = Vec::with_capacity(count);
        for _ in 0..count {
            pages.push(PageBytes {
                key: input.string(WHAT)?,
                page: page_number(input, WHAT)?,
                // ALLOC: one page's bytes, at most the bytes left in the frame.
                bytes: input.bytes_reference(WHAT)?.to_vec(),
            });
        }
        Ok(Self { pages })
    }
}
