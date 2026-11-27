//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use lucene_search::Clause;
use lucene_search::extended_query::{ExtendedQuery, MultiTermSource};
use lucene_util::automaton::{AutomatonType, CompiledAutomaton};

use super::wildcard_automaton;

fn accepts(pattern: &str, text: &str) -> bool {
    let a = wildcard_automaton(pattern);
    let compiled = CompiledAutomaton::with_options(&a, false, true, false).expect("compiles");
    match compiled.automaton_type {
        AutomatonType::NONE => false,
        AutomatonType::ALL => true,
        AutomatonType::SINGLE => compiled.term.as_deref() == Some(text.as_bytes()),
        AutomatonType::NORMAL => compiled
            .byte_runnable()
            .is_some_and(|r| r.run(text.as_bytes())),
    }
}

#[test]
fn a_wildcard_pattern_matches_in_either_case() {
    assert!(accepts("M*A?", "manual"));
    assert!(accepts("m*a?", "MANUAL"));
    assert!(!accepts("M*A?", "animal"));
    assert!(accepts("a\\*b", "A*B"));
    assert!(!accepts("a\\*b", "AxB"));
    assert!(accepts("ab\\", "AB\\"));
    assert!(accepts("*", ""));
}

#[test]
fn a_built_clause_is_an_automaton_multi_term_query() {
    let c = super::automaton_clause("f", wildcard_automaton("a"));
    let Clause::Extended(e) = c else {
        panic!("not extended")
    };
    assert!(matches!(
        *e,
        ExtendedQuery::MultiTerm(ref q) if matches!(q.source, MultiTermSource::Automaton(_))
    ));
}
