//! What a reader answers a joining reader's warm-copy asks with: the pages
//! its cache holds hot for the shards the joiner would serve, and those
//! pages' bytes.
//!
//! Both answers come from the page cache's disk tier alone (`resident_pages`
//! and `peek_page`), so a peer makes no object-store request for a joiner.
//! A key is answered only when it sits under the namespace of a shard this
//! node holds, whatever the request names.

use lucene_store::page_cache::PageCache;
use oss_transport::actions::tail::ShardRef;
use oss_transport::actions::warm::{
    HotFile, HotPage, HotSetResponse, PageBytes, PagesRequest, PagesResponse,
};

/// A shard this node holds open as a reader.
pub struct HeldShard {
    /// The shard as the wire names it.
    pub shard: ShardRef,
    /// What its files' cache keys start with: `{base}/{index}/{shard}/t=`.
    pub prefix: String,
}

/// A page ask named a key outside every shard this node holds.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused;

/// The file name and tag of a plain (`p`) cache key
/// `{namespace}\0{name}\0{stored_len}\0p[\0v..]`; `None` for any other.
fn plain_name(key: &str) -> Option<&str> {
    let mut parts = key.split('\0');
    let (_, name, _, tag) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    (tag == "p").then_some(name)
}

/// The hot pages of the shards of `held` that `serves` accepts, at most
/// `max_shards` of them and `max_bytes` stored bytes in all, each shard's
/// pages most read first. Files that are pinned or decoded are left out.
pub fn hot_set(
    cache: &PageCache,
    held: &[HeldShard],
    serves: impl Fn(&ShardRef) -> bool,
    max_shards: usize,
    max_bytes: u64,
) -> HotSetResponse {
    let page_size = cache.config().page_size;
    // ALLOC: at most `max_shards` entries.
    let chosen: Vec<&HeldShard> = held
        .iter()
        .filter(|h| serves(&h.shard))
        .take(max_shards)
        .collect();
    // ALLOC: one prefix per chosen shard.
    let prefixes: Vec<&str> = chosen.iter().map(|h| h.prefix.as_str()).collect();
    // No shard can name more pages than the byte total holds at one page each.
    let per_shard = usize::try_from(max_bytes)
        .unwrap_or(usize::MAX)
        .checked_div(page_size.max(1))
        .unwrap_or(0)
        .saturating_add(1);
    let lists = cache.resident_pages(&prefixes, per_shard);
    let mut left = max_bytes;
    // ALLOC: one entry per chosen shard.
    let mut shards = Vec::with_capacity(chosen.len());
    for (shard, pages) in chosen.iter().zip(lists) {
        // ALLOC: one entry per file listed, at most `per_shard` pages in all.
        let mut files: Vec<HotFile> = Vec::new();
        for page in pages {
            let Some(name) = plain_name(&page.key) else {
                continue;
            };
            let len = u64::from(page.len);
            if cache.is_pinned_name(name) || len > left {
                continue;
            }
            left = left.saturating_sub(len);
            let hot = HotPage {
                page: page.page,
                len: page.len,
            };
            match files.iter_mut().find(|f| *f.key == *page.key) {
                Some(file) => file.pages.push(hot),
                None => files.push(HotFile {
                    key: page.key.to_string(),
                    pages: vec![hot],
                }),
            }
        }
        shards.push((shard.shard.clone(), files));
    }
    HotSetResponse {
        page_size: i32::try_from(page_size).unwrap_or(i32::MAX),
        shards,
    }
}

/// The pages of `request` this node still holds, in the order asked.
///
/// # Errors
///
/// [`Refused`] when a key is outside every held shard's namespace.
pub fn serve(
    cache: &PageCache,
    held: &[HeldShard],
    request: &PagesRequest,
) -> Result<PagesResponse, Refused> {
    if request
        .asks
        .iter()
        .any(|ask| !held.iter().any(|h| ask.key.starts_with(&h.prefix)))
    {
        return Err(Refused);
    }
    // ALLOC: at most one entry per ask, which the wire caps.
    let mut pages = Vec::with_capacity(request.asks.len());
    for ask in &request.asks {
        if let Some(bytes) = cache.peek_page(&ask.key, ask.page) {
            pages.push(PageBytes {
                key: ask.key.clone(),
                page: ask.page,
                // ALLOC: one page, at most the cache's page size.
                bytes: bytes.to_vec(),
            });
        }
    }
    Ok(PagesResponse { pages })
}

#[cfg(test)]
mod tests;
