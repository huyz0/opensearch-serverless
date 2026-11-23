//! The orphan-shard sweep: a backstop for a shard container whose index is
//! gone ([ADR 0028](../../../docs/adr/0028-orphan-shard-sweep.md)).
//!
//! Ported from the shell's `GarbageCollector.collectOrphanedShards`. One
//! [`Sweeper::slice`] judges the next `budget` containers under `segments/`,
//! after the cursor the shell shares: it lists them, reads the descriptors of
//! their indexes, reads the pins if any container is a candidate, claims the
//! page with one conditional swap of the cursor, and only then deletes the
//! candidates no pin names. Off unless the caller asks for a slice.

mod name;
mod pins;
mod purge;

pub use pins::{IndexPins, PinSource, PointInTimeSource, SnapshotSource};

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobStore, MAX_LIST_KEYS, SwapOutcome};
use oss_meta::DescriptorStore;

use crate::JanitorError;

/// The cursor register the shell and this sweep share, under the base: the
/// last container name, UTF-8, empty meaning start over.
pub const CURSOR_REGISTER: &str = "gc/orphan-sweep-cursor";

/// A cursor is a container name; one longer than this is not one.
const CURSOR_BYTES: usize = 1024;

/// A page that fails this many times in a row is skipped.
const SKIP_AFTER: u32 = 3;

/// How a slice behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Count what would be deleted and delete nothing
    /// ([`crate::settings::DRY_RUN`]).
    pub dry_run: bool,
    /// The most index incarnations the pins may name before the slice stops.
    pub max_pins: usize,
    /// The longest descriptor a slice reads; a longer one fails the slice.
    pub descriptor_bytes: usize,
}

/// What one slice did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slice {
    /// The containers the page listed.
    pub containers: usize,
    /// Containers whose name is not one this system wrote: left alone.
    pub left_unparsed: usize,
    /// Containers of an index that is gone or has another uuid.
    pub candidates: usize,
    /// Candidates a pin names: left alone.
    pub pinned: usize,
    /// Orphans deleted, or counted under dry run.
    pub orphans: usize,
    /// Their blobs, the register included.
    pub blobs: u64,
    /// The slice lost its swap of the cursor to another writer and stopped.
    pub lost_swap: bool,
    /// The slice reached the end of the containers and wrote the empty
    /// cursor.
    pub cycle_ended: bool,
    /// The first and last container of a page that failed too often and was
    /// claimed without being judged.
    pub skipped: Option<(String, String)>,
}

#[derive(Default)]
struct Failing {
    cursor: Option<String>,
    count: u32,
}

/// The sweep, over one deployment's `base`.
pub struct Sweeper<'a> {
    store: &'a dyn BlobStore,
    descriptors: &'a DescriptorStore,
    base: String,
    config: Config,
    // No constructor takes fewer than both: there is no "no pins".
    point_in_time: &'a dyn PinSource,
    snapshots: &'a dyn PinSource,
    failing: Mutex<Failing>,
    pages_skipped: AtomicU64,
}

/// One page of containers: what was listed and where it ends.
struct Page {
    names: Vec<String>,
    last: Option<String>,
    more: bool,
}

impl<'a> Sweeper<'a> {
    /// A sweep of the deployment under `base` (empty, or ending in `/`).
    /// Both pin sources are required.
    pub fn new(
        store: &'a dyn BlobStore,
        descriptors: &'a DescriptorStore,
        base: &str,
        config: Config,
        point_in_time: &'a dyn PinSource,
        snapshots: &'a dyn PinSource,
    ) -> Self {
        Self {
            store,
            descriptors,
            // ALLOC: the base, once a sweeper.
            base: base.to_owned(),
            config,
            point_in_time,
            snapshots,
            failing: Mutex::new(Failing::default()),
            pages_skipped: AtomicU64::new(0),
        }
    }

    /// How many pages were skipped after failing three times in a row.
    pub fn pages_skipped(&self) -> u64 {
        self.pages_skipped.load(Ordering::Relaxed)
    }

    fn cursor_key(&self) -> String {
        format!("{}{CURSOR_REGISTER}", self.base)
    }

    fn segments(&self) -> String {
        format!("{}segments/", self.base)
    }

    /// Runs one slice over at most `budget` containers (never more than one
    /// LIST page, 1,000).
    ///
    /// Cost, for a page of `c` containers over `i` distinct indexes: one GET
    /// of the cursor, `c` LISTs of one key (and one more when the page is
    /// short), `i` GETs of descriptors, one conditional PUT of the cursor, and
    /// when a container is a candidate the pins' reads before the swap.
    ///
    /// # Errors
    ///
    /// [`JanitorError`] for a budget of 0 or anything that fails before the
    /// delete: a store failure, a descriptor or pin that cannot be read, pins
    /// over their cap. Nothing is deleted and the cursor does not move. The
    /// third failure in a row of the same page skips it instead.
    pub fn slice(&self, budget: usize) -> Result<Slice, JanitorError> {
        if budget == 0 {
            return Err(JanitorError::Refused(
                "an orphan slice needs a budget above 0",
            ));
        }
        let key = self.cursor_key();
        let mut buf = [0u8; CURSOR_BYTES];
        // The cursor, as the shell writes it.
        let (generation, cursor) = match self.store.read_register(&key, &mut buf)? {
            None => (ABSENT_GENERATION, String::new()),
            Some(read) => {
                let bytes = buf.get(..read.len).unwrap_or_default();
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| JanitorError::Pins("the orphan cursor is not UTF-8"))?;
                // ALLOC: one container name.
                (read.generation, text.to_owned())
            }
        };
        let page = self.next_page(&cursor, budget.min(MAX_LIST_KEYS))?;
        match self.judge(&key, generation, &page) {
            Ok(done) => {
                self.failing.lock().unwrap_or_else(|e| e.into_inner()).count = 0;
                Ok(done)
            }
            Err(error) => self.failed(&key, generation, &cursor, &page, error),
        }
    }

    fn next_page(&self, cursor: &str, budget: usize) -> Result<Page, JanitorError> {
        let segments = self.segments();
        let start = (!cursor.is_empty()).then(|| format!("{segments}{cursor}/"));
        // ALLOC: one name a container of the page, at most the budget.
        let mut names = Vec::new();
        let mut last = None;
        // LIST: one key a container, at most `budget`, resuming strictly
        // after the cursor.
        let page = self
            .store
            .list_children(&segments, start.as_deref(), budget, &mut |child| {
                let name = child
                    .strip_prefix(&segments)
                    .map(|rest| rest.strip_suffix('/').unwrap_or(rest));
                if let Some(name) = name {
                    // ALLOC: one name, as above.
                    names.push(name.to_owned());
                    // ALLOC: the last name.
                    last = Some(name.to_owned());
                }
            })?;
        Ok(Page {
            names,
            last,
            more: page.truncated,
        })
    }

    /// Everything after the listing: descriptors, pins, the claim, the deletes.
    fn judge(&self, key: &str, generation: u64, page: &Page) -> Result<Slice, JanitorError> {
        let mut done = Slice {
            containers: page.names.len(),
            ..Slice::default()
        };
        // ALLOC: one descriptor buffer, `descriptor_bytes`.
        let mut buf = vec![0u8; self.config.descriptor_bytes];
        // ALLOC: one entry a distinct index of the page, at most the budget.
        let mut live: BTreeMap<&str, Option<String>> = BTreeMap::new();
        // ALLOC: one entry a container that parses, at most the budget.
        let mut parsed = Vec::new();
        for name in &page.names {
            match name::parse(name) {
                None => done.left_unparsed = done.left_unparsed.saturating_add(1),
                Some(container) => {
                    if !live.contains_key(container.index) {
                        let uuid = self
                            .descriptors
                            .get(container.index, &mut buf)?
                            // ALLOC: one uuid a distinct index.
                            .map(|record| record.uuid.into_owned());
                        live.insert(container.index, uuid);
                    }
                    parsed.push((name.as_str(), container));
                }
            }
        }
        // ALLOC: one entry a candidate, at most the budget.
        let candidates: Vec<(&str, &name::Container<'_>)> = parsed
            .iter()
            .filter(|(_, c)| live.get(c.index).and_then(Option::as_deref) != Some(c.uuid))
            .map(|(name, c)| (*name, c))
            .collect();
        done.candidates = candidates.len();
        let mut pins = IndexPins::new(self.config.max_pins);
        if !candidates.is_empty() {
            self.point_in_time.collect(&mut pins)?;
            self.snapshots.collect(&mut pins)?;
        }
        if !self.claim(key, generation, page)? {
            done.lost_swap = true;
            return Ok(done);
        }
        done.cycle_ended = !page.more;
        let segments = self.segments();
        for (name, container) in candidates {
            if pins.contains(container.index, container.uuid) {
                done.pinned = done.pinned.saturating_add(1);
                continue;
            }
            let blobs = purge::purge(
                self.store,
                &format!("{segments}{name}/"),
                self.config.dry_run,
            )?;
            done.orphans = done.orphans.saturating_add(1);
            done.blobs = done.blobs.saturating_add(blobs);
        }
        Ok(done)
    }

    /// Moves the cursor past the page with one conditional write: the last
    /// container, or empty when the page ends the cycle. `false` when
    /// another writer moved it first.
    fn claim(&self, key: &str, generation: u64, page: &Page) -> Result<bool, JanitorError> {
        let value = match (&page.last, page.more) {
            (Some(last), true) => last.as_bytes(),
            _ => &[],
        };
        let outcome = if generation == ABSENT_GENERATION {
            self.store.create_register(key, value)?
        } else {
            self.store.swap_register(key, generation, value)?
        };
        Ok(matches!(outcome, SwapOutcome::Applied(_)))
    }

    /// A failed page counts against its cursor; the third failure in a row
    /// claims the page without judging it.
    fn failed(
        &self,
        key: &str,
        generation: u64,
        cursor: &str,
        page: &Page,
        error: JanitorError,
    ) -> Result<Slice, JanitorError> {
        let skip = {
            let mut failing = self.failing.lock().unwrap_or_else(|e| e.into_inner());
            if failing.cursor.as_deref() == Some(cursor) {
                failing.count = failing.count.saturating_add(1);
            } else {
                // ALLOC: one cursor.
                failing.cursor = Some(cursor.to_owned());
                failing.count = 1;
            }
            failing.count >= SKIP_AFTER
        };
        let (Some(first), Some(last)) = (page.names.first(), page.last.as_ref()) else {
            return Err(error);
        };
        if !skip {
            return Err(error);
        }
        // The first failure's swap may have applied before the error: a
        // lost claim only means someone else moved the page on.
        let claimed = self.claim(key, generation, page).unwrap_or(false);
        let mut failing = self.failing.lock().unwrap_or_else(|e| e.into_inner());
        failing.count = 0;
        drop(failing);
        if !claimed {
            return Err(error);
        }
        self.pages_skipped.fetch_add(1, Ordering::Relaxed);
        Ok(Slice {
            containers: page.names.len(),
            cycle_ended: !page.more,
            // ALLOC: the first and last name of the skipped page.
            skipped: Some((first.clone(), last.clone())),
            ..Slice::default()
        })
    }
}
