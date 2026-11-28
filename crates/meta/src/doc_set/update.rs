//! A change that is computed from the document it replaces.

use oss_format::MAX_DOCS;

use super::{ATTEMPTS, DocSet, Read, validate_doc_name};
use crate::error::MetaError;
use crate::pit::new_id;

/// What the rebuild closure of [`DocSet::update`] answers: the new bytes, or
/// `None` to leave the document as it is.
pub type Rebuilt = Result<Option<Vec<u8>>, MetaError>;

impl DocSet {
    /// Replaces the document `name` with what `rebuild` makes of the bytes it
    /// has now (`None` when there is none). Each attempt reads the marker and
    /// the document afresh and calls `rebuild` again, so a change that lost
    /// the swap to another writer is rebuilt on top of that writer's, not
    /// written over it. A rebuild that answers `None` writes nothing. A
    /// change that lands is 2 requests to read (1 for a set without the name)
    /// and 2 PUTs, the replaced bytes going afterwards.
    ///
    /// # Errors
    ///
    /// As [`DocSet::put`], and whatever `rebuild` answers.
    pub fn update(
        &self,
        name: &str,
        marker_buf: &mut [u8],
        body_buf: &mut [u8],
        rebuild: &mut dyn FnMut(Option<&[u8]>) -> Rebuilt,
    ) -> Result<(), MetaError> {
        validate_doc_name(name, self.kind)?;
        let mut written: Option<String> = None;
        for _ in 0..ATTEMPTS {
            let Read {
                mut marker,
                generation,
            } = self.read(marker_buf)?;
            let previous = marker.content(name).map(str::to_owned);
            let current = match &previous {
                Some(content) => match self.read_content(content, body_buf)? {
                    Some(len) => Some(len),
                    // The bytes went with a change since the marker was read.
                    None => continue,
                },
                None => None,
            };
            let old = current.and_then(|len| body_buf.get(..len));
            let Some(new) = rebuild(old)? else {
                self.drop_unnamed(written.as_deref());
                return Ok(());
            };
            if new.len() > self.kind.max_doc_bytes() {
                self.drop_unnamed(written.as_deref());
                return Err(MetaError::InvalidArgument(
                    "a document is over its size bound",
                ));
            }
            if previous.is_none() && marker.entries.len() >= MAX_DOCS {
                self.drop_unnamed(written.as_deref());
                return Err(MetaError::TooManyDocs);
            }
            // The bytes of a lost attempt are not what this one rebuilt.
            self.drop_unnamed(written.take().as_deref());
            let fresh = new_id()?;
            self.write_content(&fresh, &new)?;
            written = Some(fresh.clone());
            marker.insert(name, &fresh);
            marker.version = marker.version.saturating_add(1);
            if self.swap(&marker, generation).inspect_err(|_| {
                self.drop_unnamed(Some(&fresh));
            })? {
                if let Some(old) = previous
                    && !marker.shares(&old)
                {
                    self.drop_content(&old);
                }
                return Ok(());
            }
        }
        self.drop_unnamed(written.as_deref());
        Err(MetaError::Contended)
    }
}
