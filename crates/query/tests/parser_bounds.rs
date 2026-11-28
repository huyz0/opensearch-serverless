//! AC-2: the parser is bounded and never panics.

use oss_query::{Limits, parse};

#[test]
fn a_statement_past_the_byte_bound_is_refused_with_the_plugins_shape() {
    let sql = format!("SELECT * FROM a WHERE x = '{}'", "y".repeat(70_000));
    let err = parse(&sql, &Limits::default()).unwrap_err();
    assert_eq!(err.status(), 400);
    let body = err.to_json();
    assert_eq!(body["status"], 400);
    assert_eq!(body["error"]["type"], "SyntaxCheckException");
}

#[test]
fn token_depth_and_identifier_bounds() {
    let limits = Limits::default();
    let many = format!("SELECT * FROM a WHERE x IN ({})", vec!["1"; 5000].join(","));
    assert!(parse(&many, &limits).is_err());
    let deep = format!(
        "SELECT * FROM a WHERE {}x = 1{}",
        "(".repeat(100),
        ")".repeat(100)
    );
    assert!(parse(&deep, &limits).is_err());
    let long = format!("SELECT * FROM {}", "a".repeat(300));
    assert!(parse(&long, &limits).is_err());
}

#[test]
fn syntax_errors_name_the_position() {
    let err = parse("SELECT * FROM", &Limits::default()).unwrap_err();
    assert_eq!(err.status(), 400);
    let err = parse("select from", &Limits::default()).unwrap_err();
    // The plugin's text: the token in capitals and the position its text ends at.
    assert_eq!(
        err.to_json()["error"]["details"],
        "ERROR. token : FROM, pos : 11"
    );
    assert_eq!(
        parse("SELECT a FROM b WHERE", &Limits::default())
            .unwrap_err()
            .to_json()["error"]["type"],
        "EOFParserException"
    );
    assert_eq!(
        parse("  ", &Limits::default()).unwrap_err().to_json()["error"]["details"],
        "Query is empty"
    );
}

/// A fixed-seed mutation fuzz: no input panics, whatever it holds.
#[test]
fn no_mutation_of_a_statement_panics() {
    let seeds = [
        "SELECT firstname, age FROM accounts WHERE age BETWEEN 20 AND 30 AND city IN ('a','b') ORDER BY age DESC LIMIT 5 OFFSET 2",
        "SELECT * FROM `my-index` WHERE match(address, 'x y', operator='AND') OR NOT city LIKE 'a%'",
        "SHOW TABLES LIKE 'logs-%'",
        "DESCRIBE TABLES LIKE accounts COLUMNS LIKE 'a%'",
    ];
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = b"()'`\" ,.*=<>!%_-/\\\n\t0123456789abcXYZ[]^";
    for seed in seeds {
        for _ in 0..4000 {
            let mut bytes = seed.as_bytes().to_vec();
            for _ in 0..(next() % 6) {
                let len = u64::try_from(bytes.len()).unwrap().max(1);
                let at = usize::try_from(next() % len).unwrap();
                match next() % 3 {
                    0 => bytes.truncate(at),
                    1 if at < bytes.len() => {
                        bytes[at] = alphabet[usize::try_from(
                            next() % u64::try_from(alphabet.len()).unwrap(),
                        )
                        .unwrap()];
                    }
                    _ => bytes.insert(
                        at.min(bytes.len()),
                        alphabet[usize::try_from(next() % u64::try_from(alphabet.len()).unwrap())
                            .unwrap()],
                    ),
                }
            }
            if let Ok(text) = String::from_utf8(bytes) {
                let _ = parse(&text, &Limits::default());
            }
        }
    }
}
