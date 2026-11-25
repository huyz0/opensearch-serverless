//! Opening a shard of an index for a search: the pool charge, the commit
//! it is open on, and the errors an open answers.

use std::sync::{Arc, Mutex};

use oss_blob::{BlobError, BlobStore};
use oss_mem::{Pool, Reservation};
use oss_reader::commit_push::ShardKey;
use oss_reader::failure::{Failure, Meta, SearchError};
use oss_reader::open_cost::open_bytes;
use oss_reader::{ReaderError, ShardFiles, open_located_with};
use oss_rest::RestError;

use super::{Answer, Held, SearchService, lock_pushed};
use crate::open_indexes::{Found, Lead};

impl<S: BlobStore + 'static> SearchService<S> {
    /// Reserves `bytes` from `open_indexes`, closing the least recently
    /// searched indexes no search holds until it fits; refused when none is
    /// left to close.
    fn charge(&self, bytes: usize) -> Result<Reservation, oss_mem::Exhausted> {
        loop {
            match self.budget.try_reserve(Pool::OpenIndexes, bytes) {
                Ok(charge) => return Ok(charge),
                Err(e) => {
                    let closed = {
                        let mut set = self.lock();
                        // Closing what cannot make room would only cost
                        // those indexes a cold open; an index larger than
                        // the whole pool never fits.
                        if !closing_makes_room(&e, set.idle_bytes(|held| held.charge.bytes())) {
                            return Err(e);
                        }
                        set.close_oldest()
                    };
                    // A closed index is held by no search: its engine state
                    // and charge go here, with the set's lock released.
                    if closed.is_none() {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// The open shard 0 of `index`, opened now if it is not yet, for an
    /// index of one shard: the HTTP search serves no other.
    pub(super) fn shard(&self, index: &str) -> Result<Arc<Held<S>>, Answer> {
        self.open_shard(index, 0, true)
    }

    /// Opens shard `shard` of `index` now if it is not open, charged to the
    /// `open_indexes` pool, and keeps it open.
    ///
    /// # Errors
    ///
    /// The search error an open of it answers: no such index, a shard the
    /// index does not have, a failed read or a full pool.
    pub fn open(&self, index: &str, shard: u32) -> Result<(), Box<SearchError>> {
        self.open_shard(index, shard, false)
            .map(|_| ())
            .map_err(into_error)
    }

    /// The open shard `shard` of `index`, opened now if it is not yet;
    /// `single` refuses an index of more than one shard.
    pub(super) fn open_shard(
        &self,
        index: &str,
        shard: u32,
        single: bool,
    ) -> Result<Arc<Held<S>>, Answer> {
        let key = set_key(index, shard);
        loop {
            // The lock is released before waiting.
            let found = self.lock().find(&key);
            match found {
                Found::Open(held) => return Ok(held),
                Found::Wait(opening) => opening.wait(),
                Found::Lead => break,
            }
        }
        let lead = Lead::new(&self.open, &key);
        let (files, meta, unpublished) = match self.find_commit(index, shard) {
            Ok(found) => found,
            Err(ReaderError::Format(oss_format::FormatError::NotFollowable {
                reason: "no such shard",
                ..
            })) => return Err(out_of_range(index, shard)),
            Err(e) => return Err(open_failure(index, e)),
        };
        if single && meta.number_of_shards != 1 {
            // ALLOC: error path; one box.
            return Err(Answer::Search(Box::new(SearchError::request(
                400,
                Failure::plain(
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!(
                        "searching an index of [{}] shards is not supported by this node yet",
                        meta.number_of_shards
                    ),
                ),
            ))));
        }
        if shard >= meta.number_of_shards {
            return Err(out_of_range(index, shard));
        }
        let held = self.open_held(index, shard, files, meta, true, unpublished)?;
        Ok(lead.opened(held))
    }

    /// Charges `files`' open to the `open_indexes` pool and opens them as
    /// shard `shard` of `index`; `overlays` says whether a search of it
    /// reads the writer's overlay.
    pub(super) fn open_held(
        &self,
        index: &str,
        shard: u32,
        files: ShardFiles,
        meta: oss_reader::IndexMeta,
        overlays: bool,
        unpublished: bool,
    ) -> Result<Arc<Held<S>>, Answer> {
        let stage = files.stage().clone();
        // An index that pins analyzers needs their modules; a node that
        // cannot load them refuses it and never serves it with the standard
        // analyzer.
        let pins = oss_reader::pin_settings::pins_of(&meta.settings)
            .map_err(|e| open_failure(index, e))?;
        let opener = match (pins.is_empty(), self.modules.get()) {
            (true, _) => None,
            (false, Some(opener)) => Some(opener),
            (false, None) => {
                return Err(unavailable(
                    "this node cannot run the analyzer modules the index pins".to_owned(),
                ));
            }
        };
        let bytes = open_bytes(&files, &meta, &self.cache);
        let charge = self.charge(bytes).map_err(|e| {
            Answer::Rest(RestError::breaker(
                &e,
                "open_indexes",
                "<open_index>",
                never_fits(&e),
            ))
        })?;
        // The prefetch's bytes are held for the open alone.
        let (prefetch, _prefetch_held) = self.open_prefetch(&files, &meta);
        // The modules load beside the commit's files: a module's key comes
        // from the pins, not from anything the files hold, so a pinned index
        // opens in the rounds an unpinned one does.
        let (opened, loaded) = std::thread::scope(|scope| {
            let loading = opener.map(|o| scope.spawn(move || o.open(index, pins)));
            let opened = open_located_with(
                Arc::clone(&self.store),
                Arc::clone(&self.cache),
                &self.base,
                index,
                shard,
                files,
                meta,
                &prefetch,
            );
            (opened, loading.map(std::thread::ScopedJoinHandle::join))
        });
        let opened = opened.map_err(|e| open_failure(index, e))?;
        let pinned = match loaded {
            None => None,
            Some(Ok(Ok(tokens))) => Some(tokens),
            Some(Ok(Err(why))) => return Err(unavailable(why)),
            Some(Err(_)) => return Err(unavailable("loading the modules panicked".to_owned())),
        };
        // ALLOC: one open shard; the pool bounds them.
        Ok(Arc::new(Held {
            shard: opened,
            view: Mutex::new(None),
            pinned,
            stage,
            // ALLOC: one name an open shard.
            index: index.to_owned(),
            shard_no: shard,
            charge,
            source: if overlays {
                self.overlay.get().cloned()
            } else {
                None
            },
            epoch: self.epoch,
            unpublished,
        }))
    }

    /// The latest commit of shard `shard` of `index`: the one its writer pushed
    /// since this node's reader let go of an older one, which costs the
    /// descriptor's one GET; else the register's, two.
    fn find_commit(
        &self,
        index: &str,
        shard: u32,
    ) -> Result<(ShardFiles, oss_reader::IndexMeta, bool), ReaderError> {
        let key = ShardKey::new(index, shard);
        let taken = lock_pushed(&self.pushed).take(&key, self.now_millis());
        if let Some(taken) = taken
            && let Ok((files, meta)) = ShardFiles::open_pushed_with_meta(
                &self.store,
                &self.base,
                index,
                shard,
                &taken.value,
            )
            && meta.uuid == taken.uuid
        {
            return Ok((files, meta, false));
        }
        // No push, a push that is no commit, or one for an index since
        // recreated: the register decides, or the newer commit its writer
        // says it staged after it.
        let opened = match self.newest.get() {
            Some(source) => ShardFiles::open_asking_with_meta(
                &self.store,
                &self.base,
                index,
                shard,
                source.as_ref(),
            ),
            None => ShardFiles::open_with_meta(&self.store, &self.base, index, shard),
        };
        match opened {
            // The writer took the shard and its first publish has not
            // landed: what it wrote is in its tail, which the overlay and
            // the freshness check below read. Only with both installed.
            Err(ReaderError::Blob(BlobError::NotFound))
                if self.has_overlay_source() && self.fresh.get().is_some() =>
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
                ShardFiles::open_unpublished_with_meta(&self.store, &self.base, index, shard, now)
                    .map(|(files, meta)| (files, meta, true))
            }
            other => other.map(|(files, meta)| (files, meta, false)),
        }
    }
}

/// Whether closing indexes that free `idle` bytes would let the refused
/// reservation `e` fit.
pub(super) fn closing_makes_room(e: &oss_mem::Exhausted, idle: usize) -> bool {
    !never_fits(e) && idle >= e.in_use.saturating_add(e.requested).saturating_sub(e.limit)
}

/// Whether `e` asked for more than the whole pool: no retry admits it.
pub(super) fn never_fits(e: &oss_mem::Exhausted) -> bool {
    e.requested > e.limit
}

/// The open set's key for a shard: the index's name for shard 0, which is
/// the one a search of an index finds, and `name#n` for shard n (an index
/// name holds no `#`).
pub(crate) fn set_key(index: &str, shard: u32) -> std::borrow::Cow<'_, str> {
    if shard == 0 {
        std::borrow::Cow::Borrowed(index)
    } else {
        // ALLOC: one key a first open of a shard past the first.
        std::borrow::Cow::Owned(format!("{index}#{shard}"))
    }
}

/// What an open that failed answers, as a search error.
pub(super) fn into_error(answer: Answer) -> Box<SearchError> {
    match answer {
        Answer::Search(e) => e,
        Answer::Rest(e) => {
            // ALLOC: error path; one box and one message.
            Box::new(SearchError::request(
                e.status(),
                Failure::opensearch(
                    "circuit_breaking_exception",
                    "the open_indexes pool cannot hold the shard",
                ),
            ))
        }
    }
}

/// A shard the index does not have.
fn out_of_range(index: &str, shard: u32) -> Answer {
    // ALLOC: error path; one box.
    Answer::Search(Box::new(SearchError::request(
        400,
        Failure::plain(
            "illegal_argument_exception",
            // ALLOC: error path; one message.
            format!("shard [{shard}] is out of range for index [{index}]"),
        ),
    )))
}

/// What a failed locate or open answers.
/// The answer for a pinned index this node cannot serve: retryable, and
/// never the standard analyzer's.
fn unavailable(why: String) -> Answer {
    // ALLOC: error path; one box.
    Answer::Search(Box::new(SearchError::request(
        503,
        Failure::opensearch("extension_unavailable", why),
    )))
}

pub(super) fn open_failure(index: &str, e: ReaderError) -> Answer {
    match e {
        ReaderError::Blob(BlobError::NotFound) | ReaderError::Deleted => {
            // ALLOC: error path; one box.
            Answer::Search(Box::new(index_not_found(index)))
        }
        ReaderError::Restoring => {
            // ALLOC: error path; one message and one box.
            let failure = Failure::opensearch("cluster_block_exception", e.to_string());
            Answer::Search(Box::new(SearchError::request(503, failure)))
        }
        ReaderError::ReadBlocked(_) => {
            // ALLOC: error path; one message and one box.
            let failure = Failure::opensearch("cluster_block_exception", e.to_string());
            Answer::Search(Box::new(SearchError::request(403, failure)))
        }
        e => {
            // ALLOC: error path; one message.
            let failure = Failure::opensearch("exception", e.to_string());
            // ALLOC: error path; one box.
            Answer::Search(Box::new(SearchError::request(500, failure)))
        }
    }
}

/// `IndexNotFoundException` for an index no descriptor names.
fn index_not_found(index: &str) -> SearchError {
    SearchError::request(
        404,
        Failure::opensearch(
            "index_not_found_exception",
            // ALLOC: error path; one message.
            format!("no such index [{index}]"),
        )
        // ALLOC: error path; the index name, three times.
        .with_meta("index", Meta::Str(index.to_owned()))
        .with_meta("resource.id", Meta::Str(index.to_owned()))
        .with_meta("resource.type", Meta::Str("index_or_alias".to_owned()))
        .with_meta("index_uuid", Meta::Str("_na_".to_owned())),
    )
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Installs what loads the modules an index pins. False when one is
    /// installed already.
    pub fn set_modules(&self, opener: Arc<dyn crate::search_modules::ModuleOpener>) -> bool {
        self.modules.set(opener).is_ok()
    }
}
