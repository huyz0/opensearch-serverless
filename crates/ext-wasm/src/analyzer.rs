//! A module as an `analyzer`
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)):
//! whether a module may be pinned as one, and a block of texts run through it.

use std::fmt;

use oss_ext::CallError;
use oss_ext::analyzer::{MAX_OUTPUT_BYTES, Token, decode_block, encode_block, split_blocks};

use crate::module::{Module, Run};

/// The id of the point.
const POINT: &str = "analyzer";

/// Why a module cannot be pinned as an analyzer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinRefusal {
    /// The descriptor declares no `analyzer` point of the pinned major version.
    NoPoint(u32),
    /// The module imports something; an analyzer must be deterministic and
    /// import nothing.
    Imports(usize),
    /// The I/O regions are smaller than the point's ceilings.
    RegionTooSmall {
        /// What the descriptor declares.
        declared: u32,
        /// What the point needs.
        needed: u32,
    },
}

impl fmt::Display for PinRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPoint(v) => write!(f, "the module declares no `{POINT}` point of version {v}"),
            Self::Imports(n) => write!(
                f,
                "the module imports {n} functions and an analyzer imports none"
            ),
            Self::RegionTooSmall { declared, needed } => write!(
                f,
                "io_bytes is {declared} and an analyzer needs at least {needed}"
            ),
        }
    }
}

impl std::error::Error for PinRefusal {}

impl Module {
    /// The index of the `analyzer` point of major version `version`, if the
    /// descriptor declares one.
    fn analyzer_point(&self, version: u32) -> Option<usize> {
        self.descriptor()
            .points()
            .iter()
            .position(|p| p.id() == POINT && p.version().0 == version)
    }

    /// Whether the module may be pinned as an analyzer of point version
    /// `version`: it declares the point at that version, imports nothing and
    /// has regions that hold the point's ceilings.
    ///
    /// # Errors
    ///
    /// [`PinRefusal`] naming the first rule it breaks.
    pub fn check_analyzer_pin(&self, version: u32) -> Result<(), PinRefusal> {
        if self.analyzer_point(version).is_none() {
            return Err(PinRefusal::NoPoint(version));
        }
        if self.import_count() > 0 {
            return Err(PinRefusal::Imports(self.import_count()));
        }
        let needed = u32::try_from(MAX_OUTPUT_BYTES).unwrap_or(u32::MAX);
        let declared = self.descriptor().io_bytes();
        if declared < needed {
            return Err(PinRefusal::RegionTooSmall { declared, needed });
        }
        Ok(())
    }

    /// One call over `texts` (at most one block): the tokens of each, after
    /// the host's validation of the module's output.
    ///
    /// # Errors
    ///
    /// A [`CallError`]: a limit for a block over the point's ceilings or a
    /// call that ran out, busy for no capacity, malformed for an output the
    /// contract forbids, and unsupported when the module has no `analyzer`
    /// point of version 1.
    pub fn analyze_block(
        &self,
        tenant: &str,
        texts: &[&str],
    ) -> Result<Vec<Vec<Token>>, CallError> {
        self.analyze_block_as(tenant, texts, Run::Live)
    }

    /// As [`Module::analyze_block`], under the limits and admission `run`
    /// names.
    ///
    /// # Errors
    ///
    /// As [`Module::analyze_block`].
    pub fn analyze_block_as(
        &self,
        tenant: &str,
        texts: &[&str],
        run: Run,
    ) -> Result<Vec<Vec<Token>>, CallError> {
        let point = self
            .analyzer_point(1)
            .ok_or_else(|| CallError::from_status(oss_ext::Status::Unsupported, b""))?;
        let input = encode_block(texts)?;
        let out = self.call_as(tenant, point, &input, run)?;
        decode_block(texts, &out)
    }

    /// `texts` through the module, split into blocks in order and never
    /// inside a text; the tokens of every text, in order.
    ///
    /// # Errors
    ///
    /// The first block's failure, as [`Module::analyze_block`], or a limit
    /// for a text over the block ceiling.
    pub fn analyze(&self, tenant: &str, texts: &[&str]) -> Result<Vec<Vec<Token>>, CallError> {
        self.analyze_as(tenant, texts, Run::Live)
    }

    /// As [`Module::analyze`], under the limits and admission `run` names.
    ///
    /// # Errors
    ///
    /// As [`Module::analyze`].
    pub fn analyze_as(
        &self,
        tenant: &str,
        texts: &[&str],
        run: Run,
    ) -> Result<Vec<Vec<Token>>, CallError> {
        // ALLOC: one token list for each text.
        let mut all = Vec::with_capacity(texts.len());
        for range in split_blocks(texts)? {
            let block = texts
                .get(range)
                .ok_or_else(|| CallError::malformed_because("a block outside its texts"))?;
            all.extend(self.analyze_block_as(tenant, block, run)?);
        }
        Ok(all)
    }
}
