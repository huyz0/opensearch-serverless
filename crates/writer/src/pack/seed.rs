//! The one-time seeding walk of `packs/`.
//!
//! A node that left before the first Rust collector ran is in no index
//! register, so its packs would never be found. The walk lists the children
//! of `packs/` once a cluster, reads each child's ledger, and names in the
//! index every node whose ledger shows a live epoch. It is resumable:
//! `packs/index/seeded` holds the last child read, advanced by one
//! compare-and-swap a page, so a crash resumes after it and a racing
//! collector loses the swap and stops (duplicate work is one page). A done
//! register ends it for every later collector and restart.

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{PackSeed, check_node_id, pack_seed_key};

use super::{PackError, PackIndexStore, PackLedger};

/// The most a seed register read keeps: its cursor bound and prefix.
const READ_BYTES: usize = 2048;

/// What one step of the walk did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seeded {
    /// The walk was already done; nothing was listed.
    Done,
    /// A page was read and its cursor recorded.
    Page {
        /// Nodes named in the index by this page.
        named: usize,
        /// Whether this page reached the end.
        done: bool,
    },
    /// Another collector moved the register first; nothing was recorded.
    Lost,
}

/// The seeding walk of one deployment.
pub struct PackSeeder {
    store: Arc<dyn BlobStore>,
    base: String,
    index: PackIndexStore,
    page: usize,
}

impl std::fmt::Debug for PackSeeder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackSeeder")
            .field("base", &self.base)
            .field("page", &self.page)
            .finish_non_exhaustive()
    }
}

impl PackSeeder {
    /// A walk over the packs under `base`, `page` children a step (at
    /// least one).
    pub fn new(store: Arc<dyn BlobStore>, base: &str, page: usize) -> Self {
        Self {
            index: PackIndexStore::new(Arc::clone(&store), base),
            store,
            // ALLOC: one base, once a seeder.
            base: base.to_owned(),
            page: page.max(1),
        }
    }

    /// Reads one page of the walk, naming the nodes with a live epoch at
    /// `now_millis`, and records where it got to.
    ///
    /// # Errors
    ///
    /// The store's failures; a ledger that is not one. The cursor is not
    /// moved then, so the page is read again.
    pub fn step(&self, now_millis: u64) -> Result<Seeded, PackError> {
        let key = pack_seed_key(&self.base);
        let (mut generation, state) = self.read(&key)?;
        if state.as_ref().is_some_and(PackSeed::done) {
            return Ok(Seeded::Done);
        }
        let mut cursor = state.and_then(|s| s.cursor().map(str::to_owned));
        if generation == ABSENT_GENERATION {
            match self
                .store
                .swap_register(&key, generation, &PackSeed::begun().encode()?)?
            {
                SwapOutcome::Applied(next) => generation = next,
                _ => return Ok(Seeded::Lost),
            }
            cursor = None;
        }
        // ALLOC: the children of one page, at most `page` keys.
        let mut children: Vec<String> = Vec::new();
        let prefix = [self.base.as_str(), "packs/"].concat();
        // LIST: the one-time seeding walk, one LIST a child of `packs/` and
        // at most `page` of them a step; it never runs on a read or write
        // path, and a done register ends it.
        let listed =
            self.store
                .list_children(&prefix, cursor.as_deref(), self.page, &mut |child| {
                    // ALLOC: one child key, at most `page` of them.
                    children.push(child.to_owned());
                })?;
        let mut named = 0usize;
        for child in &children {
            let Some(node) = child
                .strip_prefix(prefix.as_str())
                .and_then(|rest| rest.strip_suffix('/'))
            else {
                continue;
            };
            if node != "index" && check_node_id(node).is_ok() && self.has_live_epoch(node)? {
                self.index.add(node, now_millis)?;
                named = named.saturating_add(1);
            }
        }
        let done = !listed.truncated;
        let next = match children.last() {
            Some(last) if !done => PackSeed::after(last),
            _ => PackSeed::finished(),
        };
        match self
            .store
            .swap_register(&key, generation, &next.encode()?)?
        {
            SwapOutcome::Applied(_) => Ok(Seeded::Page { named, done }),
            _ => Ok(Seeded::Lost),
        }
    }

    fn has_live_epoch(&self, node: &str) -> Result<bool, PackError> {
        let ledger = PackLedger::new(Arc::clone(&self.store), &self.base, node)?;
        Ok(ledger
            .read()?
            .epochs
            .iter()
            .any(|e| e.collected_below < e.high))
    }

    /// The register: its generation and state, or none.
    fn read(&self, key: &str) -> Result<(u64, Option<PackSeed>), PackError> {
        // ALLOC: one buffer a register read, at most READ_BYTES.
        let mut buf = vec![0u8; READ_BYTES];
        match self.store.read_register(key, &mut buf)? {
            Some(read) => Ok((
                read.generation,
                Some(PackSeed::parse(buf.get(..read.len).unwrap_or_default())?),
            )),
            None => Ok((ABSENT_GENERATION, None)),
        }
    }
}
