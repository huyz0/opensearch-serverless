//! The URL parameters of `_search` that set what a body key sets.
//!
//! Ported from `RestSearchAction.parseSearchSource` and
//! `FetchSourceContext.parseFromRestRequest`
//! (server/src/main/java/org/opensearch/rest/action/search/RestSearchAction.java) at OpenSearch
//! 3.10.0: a parameter on the URL replaces the body's value of the same setting.

use crate::dsl::error::DslError;
use crate::dsl::sort::{SortKey, push};
use crate::dsl::source::{DEFAULT_TRACK_TOTAL_HITS, SearchSource};
use crate::dsl::values::parse_int;

/// The URL parameters of a search that set a source setting; `None` for one the URL does not carry.
#[derive(Clone, Copy, Debug, Default)]
pub struct UrlParams<'a> {
    /// `track_total_hits`: `true`, `false` or a number.
    pub track_total_hits: Option<&'a str>,
    /// `terminate_after`: a number of matches a shard.
    pub terminate_after: Option<&'a str>,
    /// `explain`.
    pub explain: Option<&'a str>,
    /// `version`.
    pub version: Option<&'a str>,
    /// `seq_no_primary_term`.
    pub seq_no_primary_term: Option<&'a str>,
    /// `track_scores`.
    pub track_scores: Option<&'a str>,
    /// `_source`: `true`, `false` or a list of patterns to include.
    pub source: Option<&'a str>,
    /// `_source_includes`: a list of patterns.
    pub source_includes: Option<&'a str>,
    /// `_source_excludes`: a list of patterns.
    pub source_excludes: Option<&'a str>,
    /// `sort`: a list of `field` or `field:asc` or `field:desc`.
    pub sort: Option<&'a str>,
    /// `search_type`: only `query_then_fetch` is served.
    pub search_type: Option<&'a str>,
    /// `rest_total_hits_as_int`.
    pub rest_total_hits_as_int: Option<&'a str>,
    /// `typed_keys`.
    pub typed_keys: Option<&'a str>,
}

impl SearchSource {
    /// Sets from `params` what each names, over the body's value.
    ///
    /// # Errors
    ///
    /// [`DslError`] for a value OpenSearch refuses, or a `search_type` this node does not serve.
    pub fn apply_url(&mut self, params: &UrlParams<'_>) -> Result<(), DslError> {
        match params.search_type {
            None | Some("query_then_fetch") => {}
            Some("dfs_query_then_fetch") => {
                return Err(DslError::unsupported(
                    "the search type [dfs_query_then_fetch]",
                ));
            }
            Some(other) => {
                return Err(DslError::illegal(
                    // ALLOC: error path; one message.
                    format!("No search type for [{other}]"),
                ));
            }
        }
        if let Some(text) = params.track_total_hits {
            self.track_total_hits = match text {
                "true" => Some(u64::from(i32::MAX.cast_unsigned())),
                "false" => None,
                _ => match parse_int(text)? {
                    -1 => None,
                    n if n < 0 => {
                        return Err(DslError::unsupported("a [track_total_hits] below -1"));
                    }
                    n => Some(u64::from(n.cast_unsigned())),
                },
            };
        }
        if let Some(text) = params.terminate_after {
            self.terminate_after = u64::try_from(parse_int(text)?)
                // ALLOC: error path; one message.
                .map_err(|_| DslError::illegal("terminateAfter must be > 0".to_owned()))?;
        }
        self.apply_total_as_int(params.rest_total_hits_as_int)?;
        self.apply_typed_keys(params.typed_keys)?;
        flag(params.explain, &mut self.explain)?;
        flag(params.version, &mut self.version)?;
        flag(params.seq_no_primary_term, &mut self.seq_no_primary_term)?;
        flag(params.track_scores, &mut self.track_scores)?;
        self.apply_source(params)?;
        if let Some(text) = params.sort {
            // ALLOC: one entry a sort key, at most MAX_SORT_KEYS.
            let mut keys = Vec::new();
            for part in list(text) {
                push(&mut keys, sort_key(part)?)?;
            }
            self.sort = keys;
        }
        Ok(())
    }

    /// `typed_keys`: each aggregation's key carries its type.
    fn apply_typed_keys(&mut self, text: Option<&str>) -> Result<(), DslError> {
        flag(text, &mut self.typed_keys)
    }

    /// `rest_total_hits_as_int`: the total is a number and counted exactly, which a `track_total_hits`
    /// of a lower bound or `false` contradicts.
    fn apply_total_as_int(&mut self, text: Option<&str>) -> Result<(), DslError> {
        let mut on = false;
        flag(text, &mut on)?;
        if !on {
            return Ok(());
        }
        let exact = u64::from(i32::MAX.cast_unsigned());
        match self.track_total_hits {
            Some(n) if n == DEFAULT_TRACK_TOTAL_HITS => self.track_total_hits = Some(exact),
            Some(n) if n == exact => {}
            other => {
                return Err(DslError::illegal(
                    // ALLOC: error path; one message.
                    format!(
                        "[rest_total_hits_as_int] cannot be used if the tracking of total hits is not accurate, got {}",
                        other.map_or(-1, |n| i64::try_from(n).unwrap_or(i64::MAX))
                    ),
                ));
            }
        }
        self.total_as_int = true;
        Ok(())
    }

    /// `FetchSourceContext.parseFromRestRequest`: `_source` is a switch or the includes, and the two
    /// list parameters replace the includes and excludes.
    fn apply_source(&mut self, params: &UrlParams<'_>) -> Result<(), DslError> {
        if let Some(text) = params.source {
            match text {
                "true" | "false" => self.source.skip = text == "false",
                // ALLOC: one pattern a name, bounded by the URL.
                _ => self.source.includes = list(text).map(str::to_owned).collect(),
            }
        }
        if let Some(text) = params.source_includes {
            // ALLOC: one pattern a name, bounded by the URL.
            self.source.includes = list(text).map(str::to_owned).collect();
        }
        if let Some(text) = params.source_excludes {
            // ALLOC: one pattern a name, bounded by the URL.
            self.source.excludes = list(text).map(str::to_owned).collect();
        }
        Ok(())
    }
}

/// A `true` or `false` parameter, as `Booleans.parseBoolean` reads it.
fn flag(text: Option<&str>, into: &mut bool) -> Result<(), DslError> {
    match text {
        None => {}
        Some("true") => *into = true,
        Some("false") => *into = false,
        Some(other) => {
            return Err(DslError::illegal(
                // ALLOC: error path; one message.
                format!("Failed to parse value [{other}] as only [true] or [false] are allowed."),
            ));
        }
    }
    Ok(())
}

/// The names of a comma-separated list, without the empty ones.
fn list(text: &str) -> impl Iterator<Item = &str> {
    text.split(',').filter(|part| !part.is_empty())
}

/// One `sort` entry: the text after the last colon is the direction.
fn sort_key(part: &str) -> Result<SortKey, DslError> {
    let Some((field, direction)) = part.rsplit_once(':') else {
        return Ok(SortKey::named(part));
    };
    let ascending = match direction {
        "asc" => true,
        "desc" => false,
        other => {
            return Err(DslError::illegal(
                // ALLOC: error path; one message.
                format!("sort direction [{other}] not supported"),
            ));
        }
    };
    Ok(SortKey::named(field).with_order(ascending))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(params: UrlParams<'_>) -> Result<SearchSource, DslError> {
        let mut source = SearchSource::default();
        source.apply_url(&params)?;
        Ok(source)
    }

    fn refusal(params: UrlParams<'_>) -> String {
        format!("{:?}", applied(params).expect_err("a refusal"))
    }

    #[test]
    fn no_parameter_leaves_the_source_alone() {
        let source = applied(UrlParams::default()).unwrap();
        assert_eq!(source, SearchSource::default());
    }

    #[test]
    fn track_total_hits_is_a_switch_or_a_count() {
        let hits = |text| {
            applied(UrlParams {
                track_total_hits: Some(text),
                ..UrlParams::default()
            })
        };
        assert_eq!(hits("false").unwrap().track_total_hits, None);
        assert_eq!(hits("-1").unwrap().track_total_hits, None);
        assert_eq!(hits("7").unwrap().track_total_hits, Some(7));
        assert_eq!(
            hits("true").unwrap().track_total_hits,
            Some(u64::from(i32::MAX.cast_unsigned()))
        );
        assert!(hits("-2").is_err());
        assert!(hits("lots").is_err());
    }

    #[test]
    fn terminate_after_takes_a_count_from_zero() {
        let after = |text| {
            applied(UrlParams {
                terminate_after: Some(text),
                ..UrlParams::default()
            })
        };
        assert_eq!(after("3").unwrap().terminate_after, 3);
        assert_eq!(after("0").unwrap().terminate_after, 0);
        assert!(after("-1").is_err());
        assert!(after("x").is_err());
    }

    #[test]
    fn flags_are_true_or_false_only() {
        let source = applied(UrlParams {
            explain: Some("true"),
            version: Some("true"),
            seq_no_primary_term: Some("true"),
            track_scores: Some("true"),
            ..UrlParams::default()
        })
        .unwrap();
        assert!(
            source.explain && source.version && source.seq_no_primary_term && source.track_scores
        );
        let off = applied(UrlParams {
            explain: Some("false"),
            ..UrlParams::default()
        })
        .unwrap();
        assert!(!off.explain);
        for bad in ["yes", "TRUE", ""] {
            let said = refusal(UrlParams {
                version: Some(bad),
                ..UrlParams::default()
            });
            assert!(
                said.contains("only [true] or [false] are allowed"),
                "{said}"
            );
        }
    }

    #[test]
    fn source_is_a_switch_or_a_list_and_the_lists_replace_it() {
        let src = |source, includes, excludes| {
            applied(UrlParams {
                source,
                source_includes: includes,
                source_excludes: excludes,
                ..UrlParams::default()
            })
            .unwrap()
            .source
        };
        assert!(src(Some("false"), None, None).skip);
        assert!(src(Some("true"), None, None).is_whole());
        assert_eq!(src(Some("a,,b.*"), None, None).includes, ["a", "b.*"]);
        let both = src(Some("a"), Some("c,d"), Some("e"));
        assert_eq!(
            (both.includes, both.excludes),
            (vec!["c".into(), "d".into()], vec!["e".into()])
        );
    }

    #[test]
    fn sort_is_a_list_of_fields_with_a_direction() {
        let sort = applied(UrlParams {
            sort: Some("year:desc,,tag,_score:asc,a:b:asc"),
            ..UrlParams::default()
        })
        .unwrap()
        .sort;
        assert_eq!(sort.len(), 4);
        assert!(
            matches!(&sort[0], SortKey::Field { field, ascending: false, .. } if field == "year")
        );
        assert!(
            matches!(&sort[1], SortKey::Field { field, ascending: true, .. } if field == "tag")
        );
        assert!(matches!(&sort[2], SortKey::Score { ascending: true }));
        assert!(matches!(&sort[3], SortKey::Field { field, .. } if field == "a:b"));
        let said = refusal(UrlParams {
            sort: Some("year:up"),
            ..UrlParams::default()
        });
        assert!(said.contains("sort direction [up] not supported"), "{said}");
    }

    #[test]
    fn a_total_as_a_number_needs_an_exact_count() {
        let with = |track: Option<u64>, flag: &str| {
            let mut source = SearchSource::default();
            if let Some(n) = track {
                source.track_total_hits = Some(n);
            }
            source
                .apply_url(&UrlParams {
                    rest_total_hits_as_int: Some(flag),
                    ..UrlParams::default()
                })
                .map(|()| source)
        };
        let exact = u64::from(i32::MAX.cast_unsigned());
        let source = with(None, "true").unwrap();
        assert!(source.total_as_int);
        assert_eq!(source.track_total_hits, Some(exact));
        assert!(with(Some(exact), "true").unwrap().total_as_int);
        assert!(!with(None, "false").unwrap().total_as_int);
        let said = format!("{:?}", with(Some(3), "true").unwrap_err());
        assert!(said.contains("not accurate, got 3"), "{said}");
        assert!(with(None, "maybe").is_err());
        let mut off = SearchSource {
            track_total_hits: None,
            ..SearchSource::default()
        };
        let said = off
            .apply_url(&UrlParams {
                rest_total_hits_as_int: Some("true"),
                ..UrlParams::default()
            })
            .unwrap_err();
        assert!(format!("{said:?}").contains("got -1"));
    }

    #[test]
    fn typed_keys_is_a_flag() {
        let typed = |source: &mut SearchSource| {
            source.apply_url(&UrlParams {
                typed_keys: Some("true"),
                ..UrlParams::default()
            })
        };
        let mut none = SearchSource::default();
        typed(&mut none).unwrap();
        assert!(none.typed_keys);
        let mut off = SearchSource::default();
        off.apply_url(&UrlParams {
            typed_keys: Some("false"),
            ..UrlParams::default()
        })
        .unwrap();
        assert!(!off.typed_keys);
    }

    #[test]
    fn only_the_query_then_fetch_search_type_is_served() {
        let kind = |text| {
            applied(UrlParams {
                search_type: Some(text),
                ..UrlParams::default()
            })
        };
        assert!(kind("query_then_fetch").is_ok());
        assert!(
            format!("{:?}", kind("dfs_query_then_fetch").unwrap_err()).contains("not supported")
        );
        assert!(format!("{:?}", kind("scan").unwrap_err()).contains("No search type for [scan]"));
    }
}
