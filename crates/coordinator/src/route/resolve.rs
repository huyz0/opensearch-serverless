//! Turns the names in a request into the indices to fan out over: exact
//! names, prefix patterns and aliases.
//!
//! A comma list costs one metadata read a name and no listing; a prefix
//! pattern costs one capped listing, so a request never costs the size of
//! the deployment. A pattern that is not a prefix is refused, and so is one
//! that matches more than the cap, rather than answered from a subset.

use std::collections::HashSet;

/// What a name's register holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<D> {
    /// No register, or a tombstone.
    Absent,
    /// An index and its descriptor.
    Index(D),
    /// An alias and the indices it names, in its order.
    Alias(Vec<String>),
}

/// Why a prefix listing gave no names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixError {
    /// More than the cap match; the message says so.
    TooMany(String),
    /// The listing could not be read.
    Unavailable(String),
}

/// A metadata read that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError(pub String);

/// What the metadata plane gives a resolution.
pub trait Source {
    /// What an index carries that a fan-out needs, such as its shard count.
    type Desc: Clone;

    /// What `name`'s register holds, through the routing cache.
    ///
    /// # Errors
    ///
    /// A register that cannot be read.
    fn resolve(&self, name: &str) -> Result<Lookup<Self::Desc>, SourceError>;

    /// The names that start with `prefix`, in order, at most `cap`.
    ///
    /// # Errors
    ///
    /// [`PrefixError::TooMany`] past the cap.
    fn names_with_prefix(&self, prefix: &str, cap: usize) -> Result<Vec<String>, PrefixError>;

    /// The descriptor of `index`, if it exists.
    fn describe(&self, index: &str) -> Option<Self::Desc>;

    /// Whether `name` belongs to a plugin.
    fn is_system_index(&self, name: &str) -> bool;

    /// The most indices a pattern may match.
    fn pattern_cap(&self) -> usize;
}

/// How the caller wants names resolved.
#[derive(Debug, Clone, Copy)]
pub struct Request {
    /// Whether a name that is not there is an expectation rather than a
    /// mistake.
    pub ignore_unavailable: bool,
    /// Whether a plugin's index is out of reach for this caller.
    pub through_request_path: bool,
}

/// The indices a request names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<D> {
    /// Each index and its descriptor, in request order, once each.
    pub indices: Vec<(String, D)>,
    /// Names `ignore_unavailable` dropped, so the answer can say so.
    pub skipped: Vec<String>,
}

/// A refusal: the status, the error type and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The HTTP status.
    pub status: u16,
    /// The error type.
    pub kind: &'static str,
    /// The reason.
    pub reason: String,
}

impl Refusal {
    fn new(status: u16, kind: &'static str, reason: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            reason: reason.into(),
        }
    }

    fn not_found(reason: impl Into<String>) -> Self {
        Self::new(404, "index_not_found", reason)
    }

    fn system(name: &str) -> Self {
        Self::new(
            403,
            "system_index",
            // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
            format!("[{name}] belongs to a plugin and is not reachable through the request path"),
        )
    }
}

/// One trailing star and nothing else: what a prefix listing can answer. A
/// bare `*` is a prefix of everything.
fn is_prefix_pattern(name: &str) -> bool {
    name.strip_suffix('*')
        .is_some_and(|rest| !rest.contains('*'))
}

fn unavailable(message: &str) -> Refusal {
    Refusal::new(503, "metadata_unavailable", message)
}

/// Turns the comma-separated `names` into the indices to fan out over.
///
/// # Errors
///
/// A [`Refusal`] for an empty name, a pattern that is not a prefix, one that
/// matches more than the cap, a name that is not there (unless
/// `ignore_unavailable`), a plugin's index, or names none of which exist.
pub fn resolve_indices<S: Source>(
    source: &S,
    names: &str,
    request: Request,
) -> Result<Resolved<S::Desc>, Refusal> {
    // ALLOC: one entry a name in the request, bounded by its length.
    let requested: Vec<&str> = names.split(',').collect();
    for name in &requested {
        if name.is_empty() {
            return Err(Refusal::new(
                400,
                "unsupported_search",
                "an empty index name was given",
            ));
        }
        if name.contains('?') || (!is_prefix_pattern(name) && name.contains('*')) {
            return Err(Refusal::new(
                501,
                "unsupported_search",
                // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
                format!(
                    "only a trailing-wildcard pattern is supported, and [{name}] is not one: \
                     matching it would mean reading every index name in the deployment, which \
                     this system does not offer on a request path. Use a prefix like [logs-*], \
                     or name the indices you want."
                ),
            ));
        }
    }

    // Ordered and de-duplicated; what a pattern matched is kept apart, so a
    // name from a pattern can be told from one the caller typed.
    // ALLOC: one entry a name or match, bounded by the cap a pattern.
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut from_pattern: HashSet<String> = HashSet::new();
    for name in &requested {
        let Some(prefix) = name.strip_suffix('*') else {
            // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
            if seen.insert((*name).to_owned()) {
                order.push((*name).to_owned());
            }
            continue;
        };
        let matched = source
            .names_with_prefix(prefix, source.pattern_cap())
            .map_err(|e| match e {
                PrefixError::TooMany(message) => Refusal::new(400, "too_many_indices", message),
                PrefixError::Unavailable(message) => unavailable(&message),
            })?;
        for m in matched {
            from_pattern.insert(m.clone());
            if seen.insert(m.clone()) {
                order.push(m);
            }
        }
    }

    // A pattern that matches nothing is a search over no indices: refused
    // unless the caller said what is missing may be.
    for name in &requested {
        let Some(prefix) = name.strip_suffix('*') else {
            continue;
        };
        if !from_pattern.iter().any(|m| m.starts_with(prefix)) && !request.ignore_unavailable {
            // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
            return Err(Refusal::not_found(format!("no index matches [{name}]")));
        }
    }

    // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
    let mut indices: Vec<(String, S::Desc)> = Vec::new();
    let mut placed: HashSet<String> = HashSet::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut hidden: HashSet<&str> = HashSet::new();
    for name in &order {
        match source.resolve(name).map_err(|e| unavailable(&e.0))? {
            Lookup::Absent => {
                if from_pattern.contains(name) {
                    // A tombstone or an index deleted since the listing: a
                    // pattern filters what exists.
                    continue;
                }
                if request.ignore_unavailable {
                    skipped.push(name.clone());
                    continue;
                }
                // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
                return Err(Refusal::not_found(format!("no such index: {name}")));
            }
            Lookup::Index(desc) => {
                if request.through_request_path && source.is_system_index(name) {
                    if from_pattern.contains(name) {
                        hidden.insert(name.as_str());
                        continue;
                    }
                    return Err(Refusal::system(name));
                }
                if placed.insert(name.clone()) {
                    indices.push((name.clone(), desc));
                }
            }
            Lookup::Alias(targets) => {
                for target in targets {
                    let Some(desc) = source.describe(&target) else {
                        if request.ignore_unavailable {
                            skipped.push(target);
                            continue;
                        }
                        // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
                        return Err(Refusal::not_found(format!(
                            "alias [{name}] names [{target}], which does not exist"
                        )));
                    };
                    if request.through_request_path && source.is_system_index(&target) {
                        return Err(Refusal::system(&target));
                    }
                    if placed.insert(target.clone()) {
                        indices.push((target, desc));
                    }
                }
            }
        }
    }
    if indices.is_empty() {
        let visible: Vec<&str> = order
            .iter()
            .map(String::as_str)
            .filter(|n| !hidden.contains(n))
            // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
            .collect();
        if visible.is_empty() {
            return Err(Refusal::not_found(format!("no index matches [{names}]")));
        }
        // ALLOC: one name or match, bounded by the names in the request and the cap a pattern.
        return Err(Refusal::not_found(format!(
            "none of the indices named exist: {}",
            visible.join(", ")
        )));
    }
    Ok(Resolved { indices, skipped })
}
