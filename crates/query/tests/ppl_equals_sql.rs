//! docs/specs/ppl-select AC-2, AC-4, AC-5: one planner serves both languages, the PPL
//! parser is bounded and never panics, and no body holds a `script`.

#![allow(clippy::disallowed_methods)]

use oss_query::{Limits, Mapping, Statement, parse, parse_ppl, plan};
use serde_json::{Value, json};

fn mapping() -> Mapping {
    Mapping::from_json(&json!({"properties": {
        "name": {"type": "keyword"}, "age": {"type": "long"}, "city": {"type": "keyword"}}}))
    .unwrap()
}

fn body_of(sel: &oss_query::Select) -> Value {
    plan(sel, &mapping(), 200).unwrap().body().clone()
}

fn sql(text: &str) -> Value {
    let Statement::Select(s) = parse(text, &Limits::default()).unwrap() else {
        panic!("not a select");
    };
    body_of(&s)
}

fn ppl(text: &str) -> Value {
    body_of(&parse_ppl(text, &Limits::default()).unwrap())
}

// AC-2
#[test]
fn a_pipeline_lowers_to_the_body_of_its_sql_twin() {
    let pairs = [
        (
            "source=t | where age > 26 | fields name, age",
            "SELECT name, age FROM t WHERE age > 26",
        ),
        (
            "source=t | where age > 20 and city = 'x' | sort - age | head 3 | fields name",
            "SELECT name FROM t WHERE age > 20 AND city = 'x' ORDER BY age DESC LIMIT 3",
        ),
        (
            "source=t | stats count() as c by city",
            "SELECT city, count(*) AS c FROM t GROUP BY city",
        ),
        (
            "source=t | where like(name, 'a%') | fields name",
            "SELECT name FROM t WHERE name LIKE 'a%'",
        ),
        (
            "source=t | where isnull(age) | fields name",
            "SELECT name FROM t WHERE age IS NULL",
        ),
    ];
    for (p, s) in pairs {
        let (mut a, mut b) = (ppl(p), sql(s));
        // The pipeline's source filter is the only difference a column order can make.
        a["_source"] = Value::Null;
        b["_source"] = Value::Null;
        assert_eq!(a, b, "{p}");
    }
}

// AC-5
#[test]
fn no_pipeline_body_holds_a_script() {
    let b = ppl("source=t | where age > 1 | stats count() by city | sort - city");
    assert!(!b.to_string().contains("\"script\""), "{b}");
    assert!(parse_ppl("source=t | eval x = age * 2", &Limits::default()).is_err());
}

// AC-4
#[test]
fn the_parser_bounds_hold() {
    let l = Limits::default();
    let long = format!("source=t | where name = '{}'", "y".repeat(70_000));
    assert_eq!(parse_ppl(&long, &l).unwrap_err().status(), 400);
    let deep = format!(
        "source=t | where {}age = 1{}",
        "(".repeat(100),
        ")".repeat(100)
    );
    assert!(parse_ppl(&deep, &l).is_err());
    let many = format!("source=t | where age in ({})", vec!["1"; 5000].join(","));
    assert!(parse_ppl(&many, &l).is_err());
    assert!(parse_ppl(&format!("source={}", "a".repeat(300)), &l).is_err());
}

#[test]
fn no_mutation_of_a_pipeline_panics() {
    let seeds = [
        "source=compat-sql | where age > 20 and city = 'x' | sort - age | head 2 from 1 | fields name, age",
        "source=compat-sql | stats count() as c, avg(age) by city | sort - c",
        "source=compat-sql | where like(name, 'a%') or isnull(age) | fields - bio, address",
        "source=compat-sql | rename name as n | top city",
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = b"()'`\" ,.*=<>!%_-/\\\n\t|0123456789abcXYZ[]^";
    for seed in seeds {
        for _ in 0..4000 {
            let mut bytes = seed.as_bytes().to_vec();
            for _ in 0..(next() % 6) {
                let len = u64::try_from(bytes.len()).unwrap().max(1);
                let at = usize::try_from(next() % len).unwrap();
                match next() % 3 {
                    0 => bytes.truncate(at),
                    1 if at < bytes.len() => {
                        let k = usize::try_from(next() % u64::try_from(alphabet.len()).unwrap())
                            .unwrap();
                        bytes[at] = alphabet[k];
                    }
                    _ => {
                        let k = usize::try_from(next() % u64::try_from(alphabet.len()).unwrap())
                            .unwrap();
                        bytes.insert(at.min(bytes.len()), alphabet[k]);
                    }
                }
            }
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if let Ok(sel) = parse_ppl(&text, &Limits::default()) {
                let _ = plan(&sel, &mapping(), 200);
            }
        }
    }
}
