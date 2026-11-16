//! The object-store request budgets of `docs/request-budgets.md`, compiled
//! into the test binary, and the one check a gate makes against them.
//!
//! Ported from the shell's `RequestBudgets`
//! (`serverless/testkit/src/test/java/org/opensearch/serverless/testkit/RequestBudgets.java@3920dc36`).
//! The table is the one place a pinned number lives: a gate names its row
//! by key and never repeats the number, so lowering a budget is a one-line
//! change to the table, and a change that raises a count fails until the
//! table changes too, in the same diff.

use std::sync::OnceLock;

/// Where the table is, as a failure names it.
pub const TABLE: &str = "docs/request-budgets.md";

/// The table, as the build saw it.
const TEXT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/request-budgets.md"
));

/// The environment variable that turns checking into recording.
pub const RECORD: &str = "OSS_REQUEST_BUDGETS_RECORD";

/// How a measurement is held to its budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// Equal: more is a regression, less a saving the table must record.
    Exact,
    /// At most the budget.
    Max,
}

/// Whether a row is held by a gate or waits on something named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// A gate test checks it on every run.
    Enforced,
    /// Not yet met; the row names what blocks it.
    Blocked,
}

/// One row of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The objective, as a gate passes it.
    pub key: String,
    /// The case, in words.
    pub what: String,
    /// The pinned number.
    pub budget: u64,
    /// How a measurement is held to it.
    pub bound: Bound,
    /// Enforced or blocked.
    pub status: Status,
    /// For an enforced row the test that checks it; for a blocked row what
    /// blocks it.
    pub gate: String,
    /// The row's line in the table, from 1.
    pub line: usize,
}

/// Whether `s` is a `lower.case_key`.
fn is_key(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_')
}

/// One table line as a row, or why it is not one.
fn parse_row(line: &str, n: usize) -> Result<Row, String> {
    let inner = line.strip_prefix('|').unwrap_or(line);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let cells: Vec<&str> = inner.split('|').map(str::trim).collect();
    let [objective, what, budget, bound, status, gate] = cells.as_slice() else {
        return Err(format!("{n}: expected 6 cells, found {}", cells.len()));
    };
    let key = objective
        .strip_prefix('`')
        .and_then(|k| k.strip_suffix('`'))
        .filter(|k| is_key(k))
        .ok_or_else(|| format!("{n}: the objective is not a `lower.case_key`: {objective}"))?;
    let bad = || {
        format!(
            "{n}: budget must be a number, bound exact or max, status enforced or blocked: {line}"
        )
    };
    let budget = budget.replace(',', "").parse::<u64>().map_err(|_| bad())?;
    let bound = match *bound {
        "exact" => Bound::Exact,
        "max" => Bound::Max,
        _ => return Err(bad()),
    };
    let status = match *status {
        "enforced" => Status::Enforced,
        "blocked" => Status::Blocked,
        _ => return Err(bad()),
    };
    Ok(Row {
        key: key.to_owned(),
        what: (*what).to_owned(),
        budget,
        bound,
        status,
        gate: gate.replace('`', ""),
        line: n,
    })
}

/// Parses a table. A malformed row is an error naming its line, never a
/// row skipped; every error is reported at once.
///
/// # Errors
///
/// Every malformed row and repeated objective, one a line.
pub fn parse(text: &str) -> Result<Vec<Row>, String> {
    let mut rows: Vec<Row> = Vec::new();
    let mut errors = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i.saturating_add(1);
        let line = raw.trim();
        if !line.starts_with('|') || line.starts_with("|---") || line.starts_with("| Objective") {
            continue;
        }
        match parse_row(line, n) {
            Ok(row) if rows.iter().any(|r| r.key == row.key) => {
                errors.push(format!("{n}: objective {} appears twice", row.key));
            }
            Ok(row) => rows.push(row),
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(rows)
    } else {
        Err(format!("{TABLE} is malformed:\n  {}", errors.join("\n  ")))
    }
}

/// Every row of the table, in table order.
///
/// # Panics
///
/// When the table is malformed, naming each bad line.
pub fn rows() -> &'static [Row] {
    static ROWS: OnceLock<Vec<Row>> = OnceLock::new();
    ROWS.get_or_init(|| parse(TEXT).unwrap_or_else(|e| panic!("{e}")))
}

/// Whether this run records measurements instead of checking them:
/// `OSS_REQUEST_BUDGETS_RECORD=1`, for filling in a new row or reading where
/// a changed count's requests went.
pub fn recording() -> bool {
    std::env::var(RECORD).is_ok_and(|v| v == "1" || v == "true")
}

/// Holds `measured` to its row's budget. `detail` says where the requests
/// went, shown on a failure so a regression names its cause, and printed
/// in record mode.
///
/// # Panics
///
/// On a regression, an exact row's saving, an unknown key or a blocked
/// one: each a failed gate, with what to do about it.
pub fn check(key: &str, measured: u64, detail: &str) {
    if recording() {
        eprintln!("request budget {key}: measured {measured}\n{detail}");
        return;
    }
    let Some(row) = rows().iter().find(|r| r.key == key) else {
        panic!("no objective {key} in {TABLE}");
    };
    assert!(
        row.status == Status::Enforced,
        "objective {key} is blocked; a gate checks only enforced rows"
    );
    if let Some(failure) = verdict(row, measured) {
        panic!("{failure}\n{detail}");
    }
}

/// Judges `measured` against `row`: the failure, or `None` when it meets
/// the budget.
pub fn verdict(row: &Row, measured: u64) -> Option<String> {
    let Row {
        key, what, budget, ..
    } = row;
    if measured > *budget {
        return Some(format!(
            "{key} regressed: measured {measured}, budget {budget} ({what}). Find the new requests below; \
             if they are worth paying, raise the budget in {TABLE} in the same change and say why in its description."
        ));
    }
    if row.bound == Bound::Exact && measured < *budget {
        return Some(format!(
            "{key} improved: measured {measured}, budget {budget} ({what}). Lower the budget in {TABLE} to \
             {measured} so the saving cannot be lost silently."
        ));
    }
    None
}
