//! docs/specs/coordinator-index-resolution: the names in a request become the
//! indices to fan out over.

use std::collections::{BTreeMap, BTreeSet};

use oss_coordinator::route::resolve::{
    Lookup, PrefixError, Refusal, Request, Source, SourceError, resolve_indices,
};

/// An in-memory metadata plane: indices (with a shard count), aliases, system
/// indices and names the listing returns that no register describes.
#[derive(Default)]
struct Plane {
    indices: BTreeMap<String, u32>,
    aliases: BTreeMap<String, Vec<String>>,
    system: BTreeSet<String>,
    ghosts: BTreeSet<String>,
    cap: usize,
}

impl Plane {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            ..Self::default()
        }
    }

    fn index(mut self, name: &str, shards: u32) -> Self {
        self.indices.insert(name.to_owned(), shards);
        self
    }

    fn alias(mut self, name: &str, to: &[&str]) -> Self {
        self.aliases.insert(
            name.to_owned(),
            to.iter().map(|s| (*s).to_owned()).collect(),
        );
        self
    }

    fn system(mut self, name: &str) -> Self {
        self.system.insert(name.to_owned());
        self
    }

    fn ghost(mut self, name: &str) -> Self {
        self.ghosts.insert(name.to_owned());
        self
    }
}

impl Source for Plane {
    type Desc = u32;

    fn resolve(&self, name: &str) -> Result<Lookup<u32>, SourceError> {
        if let Some(shards) = self.indices.get(name) {
            return Ok(Lookup::Index(*shards));
        }
        if let Some(to) = self.aliases.get(name) {
            return Ok(Lookup::Alias(to.clone()));
        }
        Ok(Lookup::Absent)
    }

    fn names_with_prefix(&self, prefix: &str, cap: usize) -> Result<Vec<String>, PrefixError> {
        let mut found: Vec<String> = self
            .indices
            .keys()
            .chain(self.aliases.keys())
            .chain(self.ghosts.iter())
            .filter(|n| n.starts_with(prefix))
            .cloned()
            .collect();
        found.sort();
        if found.len() > cap {
            return Err(PrefixError::TooMany(format!(
                "too many indices begin with [{prefix}]"
            )));
        }
        Ok(found)
    }

    fn describe(&self, index: &str) -> Option<u32> {
        self.indices.get(index).copied()
    }

    fn is_system_index(&self, name: &str) -> bool {
        self.system.contains(name)
    }

    fn pattern_cap(&self) -> usize {
        self.cap
    }
}

fn ok(plane: &Plane, names: &str, ignore_unavailable: bool) -> (Vec<(String, u32)>, Vec<String>) {
    let request = Request {
        ignore_unavailable,
        through_request_path: true,
    };
    let resolved = resolve_indices(plane, names, request).expect("resolves");
    (resolved.indices, resolved.skipped)
}

fn refused(plane: &Plane, names: &str, ignore_unavailable: bool) -> Refusal {
    let request = Request {
        ignore_unavailable,
        through_request_path: true,
    };
    resolve_indices(plane, names, request).expect_err("refused")
}

fn pairs(list: &[(&str, u32)]) -> Vec<(String, u32)> {
    list.iter().map(|(n, s)| ((*n).to_owned(), *s)).collect()
}

#[test]
fn a_list_resolves_in_request_order_once_each_and_an_alias_expands() {
    let plane = Plane::new(10)
        .index("a", 1)
        .index("b", 2)
        .index("c", 3)
        .alias("ab", &["b", "a"]);
    let (indices, skipped) = ok(&plane, "c,ab,a,c", false);
    assert_eq!(indices, pairs(&[("c", 3), ("b", 2), ("a", 1)]));
    assert!(skipped.is_empty());
}

#[test]
fn an_empty_name_or_a_pattern_that_is_not_a_prefix_is_refused() {
    let plane = Plane::new(10).index("a", 1);
    let empty = refused(&plane, "a,,b", false);
    assert_eq!((empty.status, empty.kind), (400, "unsupported_search"));
    assert_eq!(empty.reason, "an empty index name was given");
    for name in ["a?", "*a", "a*b", "a**"] {
        let r = refused(&plane, name, false);
        assert_eq!((r.status, r.kind), (501, "unsupported_search"), "{name}");
        assert_eq!(
            r.reason,
            format!(
                "only a trailing-wildcard pattern is supported, and [{name}] is not one: \
                 matching it would mean reading every index name in the deployment, which \
                 this system does not offer on a request path. Use a prefix like [logs-*], \
                 or name the indices you want."
            )
        );
    }
    // A bare star is a prefix of everything.
    let (indices, _) = ok(&plane, "*", false);
    assert_eq!(indices, pairs(&[("a", 1)]));
}

#[test]
fn a_prefix_pattern_resolves_up_to_the_cap_and_ghosts_are_not_matches() {
    let plane = Plane::new(4)
        .index("logs-1", 1)
        .index("logs-2", 1)
        .alias("logs-all", &["logs-1"])
        .ghost("logs-0")
        .index("other", 1);
    let (indices, skipped) = ok(&plane, "logs-*", false);
    assert_eq!(indices, pairs(&[("logs-1", 1), ("logs-2", 1)]));
    assert!(skipped.is_empty(), "a ghost is a non-match, not a skip");
    let wide = Plane::new(1).index("x1", 1).index("x2", 1);
    let r = refused(&wide, "x*", false);
    assert_eq!((r.status, r.kind), (400, "too_many_indices"));
    assert_eq!(r.reason, "too many indices begin with [x]");
}

#[test]
fn a_missing_name_is_refused_or_skipped_and_nothing_resolving_is_refused() {
    let plane = Plane::new(10).index("a", 1);
    let r = refused(&plane, "a,nope", false);
    assert_eq!((r.status, r.kind), (404, "index_not_found"));
    assert_eq!(r.reason, "no such index: nope");
    let (indices, skipped) = ok(&plane, "a,nope", true);
    assert_eq!(indices, pairs(&[("a", 1)]));
    assert_eq!(skipped, ["nope"]);
    let r = refused(&plane, "p-*", false);
    assert_eq!(r.reason, "no index matches [p-*]");
    let (indices, _) = ok(&plane, "a,p-*", true);
    assert_eq!(indices, pairs(&[("a", 1)]));
    let none = refused(&plane, "x,y", true);
    assert_eq!((none.status, none.kind), (404, "index_not_found"));
    assert_eq!(none.reason, "none of the indices named exist: x, y");
}

#[test]
fn an_alias_naming_a_gone_index_is_refused_or_skips_that_index() {
    let mut plane = Plane::new(10).index("a", 1).alias("al", &["a", "gone"]);
    plane.indices.remove("gone");
    let r = refused(&plane, "al", false);
    assert_eq!((r.status, r.kind), (404, "index_not_found"));
    assert_eq!(r.reason, "alias [al] names [gone], which does not exist");
    let (indices, skipped) = ok(&plane, "al", true);
    assert_eq!(indices, pairs(&[("a", 1)]));
    assert_eq!(skipped, ["gone"]);
}

#[test]
fn a_system_index_is_refused_when_typed_and_hidden_when_a_pattern_matched_it() {
    let plane = Plane::new(10)
        .index(".sec", 1)
        .index("data", 1)
        .alias("sys", &[".sec"])
        .system(".sec");
    let r = refused(&plane, ".sec", false);
    assert_eq!((r.status, r.kind), (403, "system_index"));
    assert_eq!(
        r.reason,
        "[.sec] belongs to a plugin and is not reachable through the request path"
    );
    let r = refused(&plane, "sys", false);
    assert_eq!((r.status, r.kind), (403, "system_index"));
    // An alias a pattern matched still reaches the system index it names.
    let r = refused(&plane, "*", false);
    assert_eq!((r.status, r.kind), (403, "system_index"));
    let plain = Plane::new(10)
        .index(".sec", 1)
        .index("data", 1)
        .system(".sec");
    let (indices, _) = ok(&plain, "*", false);
    assert_eq!(indices, pairs(&[("data", 1)]));
    let only = Plane::new(10).index(".sec", 1).system(".sec");
    let r = refused(&only, ".s*", false);
    assert_eq!((r.status, r.kind), (404, "index_not_found"));
    assert_eq!(r.reason, "no index matches [.s*]");
    // Off the request path the guard does not apply.
    let request = Request {
        ignore_unavailable: false,
        through_request_path: false,
    };
    let resolved = resolve_indices(&plane, ".sec", request).expect("resolves");
    assert_eq!(resolved.indices, pairs(&[(".sec", 1)]));
}
