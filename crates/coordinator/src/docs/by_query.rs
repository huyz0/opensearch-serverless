//! Walking a delete- or update-by-query one shard at a time.
//!
//! For each shard the walk fixes an upper bound (the newest matching
//! sequence number), then pages through the matches in ascending sequence
//! order, so documents written during the walk are not chased and a shard
//! that keeps receiving matches cannot make it run forever.

/// Documents asked for in one page.
pub const BATCH_SIZE: u64 = 1000;

/// One matching document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The document id.
    pub id: String,
    /// Its sequence number: the order pages follow and the condition its
    /// change carries.
    pub seq_no: i64,
    /// Its primary term.
    pub primary_term: i64,
}

/// What a change did to one hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The document was changed.
    Changed,
    /// The change left it alone.
    Noop,
    /// The document was rewritten (or removed) after the page was read;
    /// carries the conflict's message.
    Conflict(String),
}

/// What the walk's counts came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    /// Documents visited.
    pub visited: u64,
    /// Documents changed.
    pub changed: u64,
    /// Documents the change left alone.
    pub noops: u64,
    /// Documents that conflicted.
    pub conflicts: u64,
    /// The id the walk aborted on, when it aborted on a conflict.
    pub aborted_on: Option<String>,
    /// The message of that conflict.
    pub conflict: Option<String>,
    /// Non-empty pages asked for.
    pub batches: u32,
}

/// What the walk needs from the node's search and memory accounting.
pub trait Source {
    /// A failure to search or to admit a page.
    type Error;
    /// The hold a page keeps on the node's indexing-pressure budget while
    /// its changes run; released on drop.
    type Permit<'a>
    where
        Self: 'a;

    /// The newest sequence number matching on `shard`, if anything does.
    ///
    /// # Errors
    ///
    /// A search that failed.
    fn newest(&self, shard: u32) -> Result<Option<i64>, Self::Error>;

    /// A page of at most `size` matches on `shard` with a sequence number up
    /// to `up_to`, ascending, after `after` when given.
    ///
    /// # Errors
    ///
    /// A search that failed.
    fn page(
        &self,
        shard: u32,
        up_to: i64,
        after: Option<i64>,
        size: usize,
    ) -> Result<Vec<Hit>, Self::Error>;

    /// Admits a page of `bytes` as the writes it is.
    ///
    /// # Errors
    ///
    /// The budget's refusal.
    fn admit(&self, bytes: u64) -> Result<Self::Permit<'_>, Self::Error>;
}

/// The change applied to each hit.
pub trait Change {
    /// A failure to apply.
    type Error;

    /// Changes `hit`, conditional on its sequence number and primary term;
    /// `refresh` when this is the last change of the shard.
    ///
    /// # Errors
    ///
    /// A failure other than a conflict.
    fn apply(&mut self, hit: &Hit, refresh: bool) -> Result<Applied, Self::Error>;
}

/// The bytes a page is admitted for.
fn page_bytes(hits: &[Hit]) -> u64 {
    hits.iter().fold(0u64, |sum, hit| {
        let id = u64::try_from(hit.id.len()).unwrap_or(u64::MAX);
        sum.saturating_add(64).saturating_add(id)
    })
}

/// Walks `shards` in order, changing each matching document, until
/// `max_docs` were visited.
///
/// # Errors
///
/// The first search, admission or change failure.
pub fn walk<S, C>(
    source: &S,
    shards: &[u32],
    max_docs: u64,
    refresh: bool,
    abort_on_conflict: bool,
    change: &mut C,
) -> Result<Walk, S::Error>
where
    S: Source,
    C: Change<Error = S::Error>,
{
    let mut out = Walk {
        visited: 0,
        changed: 0,
        noops: 0,
        conflicts: 0,
        aborted_on: None,
        conflict: None,
        batches: 0,
    };
    for &shard in shards {
        if out.visited >= max_docs {
            break;
        }
        let Some(up_to) = source.newest(shard)? else {
            continue;
        };
        let mut after: Option<i64> = None;
        // The shard check above and the last page's break keep `visited`
        // below `max_docs` here, so the loop ends only by those breaks.
        loop {
            let want = BATCH_SIZE.min(max_docs.saturating_sub(out.visited));
            let size = usize::try_from(want).unwrap_or(usize::MAX);
            let hits = source.page(shard, up_to, after, size)?;
            if hits.is_empty() {
                break;
            }
            out.batches = out.batches.saturating_add(1);
            let count = u64::try_from(hits.len()).unwrap_or(u64::MAX);
            let last_page = count < want || out.visited.saturating_add(count) >= max_docs;
            // A page of changes is a page of writes, accounted as one.
            let _held = source.admit(page_bytes(&hits))?;
            for (i, hit) in hits.iter().enumerate() {
                out.visited = out.visited.saturating_add(1);
                // The refresh, when asked for, rides the shard's last change.
                let last = refresh && last_page && i.saturating_add(1) == hits.len();
                match change.apply(hit, last)? {
                    Applied::Changed => out.changed = out.changed.saturating_add(1),
                    Applied::Noop => out.noops = out.noops.saturating_add(1),
                    Applied::Conflict(message) => {
                        out.conflicts = out.conflicts.saturating_add(1);
                        if abort_on_conflict {
                            out.aborted_on = Some(hit.id.clone());
                            out.conflict = Some(message);
                            return Ok(out);
                        }
                    }
                }
            }
            after = hits.last().map(|hit| hit.seq_no);
            if last_page {
                break;
            }
        }
    }
    Ok(out)
}
