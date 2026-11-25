//! One document's postings for one field, as `IndexingChain.invert` builds
//! them across the field's values: positions from -1, each value after the
//! first behind the position increment gap and an offset gap of 1, and the
//! field's length for its norm.

use std::collections::HashMap;

use lucene_analysis::Analyzer;
use lucene_codecs::field_infos::IndexOptions;
use lucene_index::index_writer::{InvertedField, InvertedTerm};
use oss_ext::analyzer::{Analyze, Token};

use super::encode::norm;
use crate::mapping::MappingError;

/// `Analyzer.getOffsetGap`'s default.
const OFFSET_GAP: i32 = 1;

/// A field's postings in one document, as its values are added.
pub struct Inverted {
    /// The field's number.
    pub number: i32,
    options: IndexOptions,
    norms: bool,
    terms: Vec<InvertedTerm>,
    /// Where each distinct term sits in `terms`.
    at: HashMap<Vec<u8>, usize>,
    position: i32,
    offset: i32,
    /// Tokens, for the norm.
    length: u32,
    /// Tokens at the position of the one before, which BM25 leaves out of
    /// the length.
    overlap: u32,
    seen: bool,
}

impl Inverted {
    /// No value yet.
    pub fn new(number: i32, options: IndexOptions, norms: bool) -> Self {
        Inverted {
            number,
            options,
            norms,
            // ALLOC: the field's distinct terms in this document, at most its tokens; bounded by the document's bytes.
            terms: Vec::new(),
            // ALLOC: as `terms`.
            at: HashMap::new(),
            position: -1,
            offset: 0,
            length: 0,
            overlap: 0,
            seen: false,
        }
    }

    fn add(&mut self, term: &[u8], offsets: Option<(i32, i32)>) {
        let i = match self.at.get(term) {
            Some(i) => *i,
            None => {
                let i = self.terms.len();
                // ALLOC: one distinct term, bounded by the document's bytes.
                self.at.insert(term.to_vec(), i);
                // ALLOC: as above.
                self.terms.push(InvertedTerm {
                    term: term.to_vec(),
                    freq: 0,
                    // ALLOC: none until a token is added: one entry a token, bounded by the document's bytes.
                    positions: Vec::new(),
                    offsets: Vec::new(),
                    payloads: Vec::new(),
                });
                i
            }
        };
        let Some(t) = self.terms.get_mut(i) else {
            return;
        };
        t.freq = t.freq.saturating_add(1);
        if self.options.subsumes_positions() {
            // ALLOC: one position a token, bounded by the document's bytes.
            t.positions.push(self.position);
        }
        if let (true, Some(o)) = (self.options.subsumes_offsets(), offsets) {
            // ALLOC: one offset pair a token, as positions.
            t.offsets.push(o);
        }
    }

    /// An untokenized value: one term (`KeywordField`, a boolean, a
    /// metadata field).
    pub fn keyword(&mut self, term: &[u8]) {
        self.seen = true;
        self.position = self.position.saturating_add(1);
        self.length = self.length.saturating_add(1);
        self.add(term, None);
    }

    /// An analyzed value, after the gap when it is not the field's first.
    ///
    /// # Errors
    ///
    /// The analyzer's failure, which the built-in standard analyzer never
    /// has over a string.
    pub fn text(&mut self, analyzer: &Analyzer, text: &str, gap: i32) -> Result<(), MappingError> {
        if self.seen {
            self.position = self.position.saturating_add(gap);
            self.offset = self.offset.saturating_add(OFFSET_GAP);
        }
        self.seen = true;
        let (end_increment, end_offset) = analyzer
            .try_for_each_token(text, |term, start, end, increment| {
                self.position = self.position.saturating_add(increment);
                if increment == 0 {
                    self.overlap = self.overlap.saturating_add(1);
                }
                self.length = self.length.saturating_add(1);
                let base = self.offset;
                self.add(
                    term.as_bytes(),
                    Some((base.saturating_add(start), base.saturating_add(end))),
                );
            })
            // ALLOC: error path; one message.
            .map_err(|e| MappingError::illegal_state(&e.to_string()))?;
        self.position = self.position.saturating_add(end_increment);
        self.offset = self.offset.saturating_add(end_offset);
        Ok(())
    }

    /// An analyzed value: the tokens of `module` when the field pins one,
    /// else those of the built-in `analyzer`.
    ///
    /// # Errors
    ///
    /// The module's failure, which is never answered by the built-in
    /// analyzer, or a position past what a posting holds.
    pub fn analyzed(
        &mut self,
        analyzer: &Analyzer,
        module: Option<&dyn Analyze>,
        text: &str,
        gap: i32,
    ) -> Result<(), MappingError> {
        let Some(module) = module else {
            return self.text(analyzer, text, gap);
        };
        let tokens = module.analyze(text).map_err(|e| {
            // ALLOC: error path; one message.
            MappingError::illegal_state(&format!(
                "the analyzer module failed ({:?}): {}",
                e.kind(),
                e.detail()
            ))
        })?;
        let units = text.encode_utf16().count();
        self.tokens(&tokens, i32::try_from(units).unwrap_or(i32::MAX), gap)
    }

    /// The tokens a pinned module made of one value of `units` UTF-16 units,
    /// after the gap when it is not the field's first. The host has checked
    /// them, so the increments and offsets are the module's own and only
    /// the gaps are the host's.
    ///
    /// # Errors
    ///
    /// A position or an offset past what a posting holds.
    pub fn tokens(&mut self, tokens: &[Token], units: i32, gap: i32) -> Result<(), MappingError> {
        if self.seen {
            self.position = self.position.saturating_add(gap);
            self.offset = self.offset.saturating_add(OFFSET_GAP);
        }
        self.seen = true;
        let base = self.offset;
        for t in tokens {
            let increment = i32::try_from(t.increment).map_err(|_| {
                MappingError::illegal_state("a token's position increment is too large")
            })?;
            let (start, end) = (i32::try_from(t.start), i32::try_from(t.end));
            let (Ok(start), Ok(end)) = (start, end) else {
                return Err(MappingError::illegal_state("a token's offset is too large"));
            };
            self.position = self.position.saturating_add(increment);
            if increment == 0 {
                self.overlap = self.overlap.saturating_add(1);
            }
            self.length = self.length.saturating_add(1);
            self.add(
                t.term.as_bytes(),
                Some((base.saturating_add(start), base.saturating_add(end))),
            );
        }
        self.offset = self.offset.saturating_add(units);
        Ok(())
    }

    /// The engine's field, or the field's number alone when no value gave
    /// a token: the field is still one of the segment's, with no postings.
    pub fn finish(self) -> Result<InvertedField, i32> {
        if self.terms.is_empty() {
            return Err(self.number);
        }
        // `BM25Similarity.computeNorm`: a field without frequencies counts
        // its distinct terms; any other its tokens, less overlaps.
        let tokens = if self.options == IndexOptions::Docs {
            u32::try_from(self.terms.len()).unwrap_or(u32::MAX)
        } else {
            self.length.saturating_sub(self.overlap)
        };
        let norm = self.norms.then(|| norm(tokens));
        Ok(InvertedField {
            field_number: self.number,
            terms: self.terms.into(),
            norm,
        })
    }
}

#[cfg(test)]
mod tests;
