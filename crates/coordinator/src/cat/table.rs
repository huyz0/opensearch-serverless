//! A cat table drawn as text for a person at a terminal, or as JSON for a
//! caller that asked for a structured format, whole or as one page of a
//! `_list` walk.

use oss_doc::mapping::json::write_str;

/// The words of the refusal when `h` selects columns.
const COLUMNS: &str = "'h' selects columns, which this implementation does not support; it answers \
     with a fixed column set, and returning that while accepting 'h' would hand back a table that \
     is not the one asked for";

/// The words of the refusal when `s` sorts.
const SORT: &str =
    "'s' sorts, which this implementation does not support; rows come back in a fixed order";

/// Why a request's `h` or `s` cannot be answered; nothing when it can.
#[must_use]
pub fn unsupported(columns: Option<&str>, sort: Option<&str>) -> Option<&'static str> {
    if columns.is_some() {
        Some(COLUMNS)
    } else if sort.is_some() {
        Some(SORT)
    } else {
        None
    }
}

/// Where a page of a `_list` walk ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page<'a> {
    /// The array's key in a structured answer.
    pub entity: &'a str,
    /// Where the next page starts; nothing on the last page.
    pub next_token: Option<&'a str>,
}

/// A table's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// Whether the body is JSON, else plain text.
    pub structured: bool,
    /// The body.
    pub body: String,
}

/// A table of `N` columns.
#[derive(Clone, Debug)]
pub struct Table<const N: usize> {
    headers: [&'static str; N],
    rows: Vec<[String; N]>,
}

impl<const N: usize> Table<N> {
    /// An empty table with these columns.
    #[must_use]
    pub fn new(headers: [&'static str; N]) -> Self {
        Self {
            headers,
            // ALLOC: empty until rows are added.
            rows: Vec::new(),
        }
    }

    /// Adds a row.
    pub fn row(&mut self, cells: [String; N]) {
        // ALLOC: one row, bounded by the rows the caller read.
        self.rows.push(cells);
    }

    /// Adds a row in which a missing cell is drawn as `null`.
    pub fn row_opt(&mut self, cells: [Option<String>; N]) {
        // ALLOC: one "null" for each missing cell.
        self.row(cells.map(|c| c.unwrap_or_else(|| "null".to_owned())));
    }

    /// The table in the format asked for (`text` or none is text, anything
    /// else is structured), with the header row when `verbose`.
    #[must_use]
    pub fn answer(&self, format: Option<&str>, verbose: bool, page: Option<Page<'_>>) -> Answer {
        match format {
            None | Some("text") => Answer {
                structured: false,
                body: self.text(verbose, page),
            },
            Some(_) => Answer {
                structured: true,
                body: self.json(page),
            },
        }
    }

    /// The table as aligned text: each cell but a row's last is padded to
    /// its column's widest plus one space; the header row only when
    /// `verbose`; a page ends with its `next_token` line.
    #[must_use]
    pub fn text(&self, verbose: bool, page: Option<Page<'_>>) -> String {
        let mut widths = [0usize; N];
        if verbose {
            for (w, h) in widths.iter_mut().zip(&self.headers) {
                *w = width(h);
            }
        }
        for row in &self.rows {
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(width(cell));
            }
        }
        // ALLOC: one string, bounded by the rows the caller read.
        let mut out = String::new();
        if verbose {
            line(&mut out, &self.headers, &widths);
        }
        for row in &self.rows {
            line(&mut out, row, &widths);
        }
        if let Some(page) = page {
            out.push_str("next_token ");
            out.push_str(page.next_token.unwrap_or("null"));
            out.push('\n');
        }
        out
    }

    /// The table as JSON: an array of objects whose fields are all strings,
    /// or for a page an object holding `next_token` and that array.
    #[must_use]
    pub fn json(&self, page: Option<Page<'_>>) -> String {
        // ALLOC: one string, bounded by the rows the caller read.
        let mut out = String::new();
        if let Some(page) = page {
            out.push_str("{\"next_token\":");
            match page.next_token {
                Some(token) => write_str(&mut out, token),
                None => out.push_str("null"),
            }
            out.push(',');
            write_str(&mut out, page.entity);
            out.push(':');
        }
        out.push('[');
        for (n, row) in self.rows.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            out.push('{');
            for (i, (header, cell)) in self.headers.iter().zip(row).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(&mut out, header);
                out.push(':');
                write_str(&mut out, cell);
            }
            out.push('}');
        }
        out.push(']');
        if page.is_some() {
            out.push('}');
        }
        out
    }
}

/// A cell's width in UTF-16 units, as Java's `String.length` counts it.
fn width(cell: &str) -> usize {
    cell.encode_utf16().count()
}

fn line<S: AsRef<str>>(out: &mut String, cells: &[S], widths: &[usize]) {
    let last = cells.len().saturating_sub(1);
    for (i, (cell, w)) in cells.iter().zip(widths).enumerate() {
        let cell = cell.as_ref();
        out.push_str(cell);
        if i < last {
            for _ in 0..w.saturating_sub(width(cell)).saturating_add(1) {
                out.push(' ');
            }
        }
    }
    out.push('\n');
}
