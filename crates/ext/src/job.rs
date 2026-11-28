//! The seam a plugin starts a search job through: `job.submit`, `job.get`,
//! `job.delete` and `job.stats` (docs/specs/plugin-system, Host API 1.0;
//! [ADR 0084](../../../docs/adr/0084-asynchronous-search-is-a-native-plugin-over-node-local-jobs.md)).
//!
//! A job is data, not a closure: a plugin names an index expression and a
//! search body, and the node runs that one search on a thread of its own. The
//! plugin never supplies code to run there. [`JobSink`] is the node's side;
//! the bytes are the codec below, so every tier calls the same operations, and
//! [`JobBackend`] serves them from a `JobSink` and hands every other operation
//! to the backend behind it.

use std::sync::Arc;

use crate::host::{Backend, HostError, Who};
use crate::rest::CodecError;

const VERSION: u8 = 1;
/// The most bytes of an index expression or a job id.
pub const MAX_NAME_BYTES: usize = 1024;
/// The most bytes of a search body.
pub const MAX_BODY_BYTES: usize = 1 << 20;
/// The most bytes of a job's result.
pub const MAX_RESULT_BYTES: usize = 4 << 20;

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Its search is running.
    Running,
    /// It finished with a result.
    Succeeded,
    /// It finished with an error body.
    Failed,
}

/// A read of a job: its state, and the result or the error body once it has
/// finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobView {
    /// The state.
    pub state: JobState,
    /// The result or the error body; empty while it runs.
    pub body: Vec<u8>,
}

/// This node's job counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobStats {
    /// Jobs whose search is running.
    pub running: u64,
    /// Jobs submitted since the node started.
    pub submitted: u64,
    /// Jobs a delete cancelled.
    pub cancelled: u64,
    /// Jobs refused at a limit.
    pub rejected: u64,
}

/// The node's side of the `job.*` operations, for the plugin `who` names.
pub trait JobSink: Send + Sync {
    /// Starts the search `body` over the index expression `index` and gives
    /// the job's id.
    ///
    /// # Errors
    ///
    /// `Budget` at a limit, `Invalid` for a search the node refuses.
    fn submit(&self, who: &Who<'_>, index: &str, body: &[u8]) -> Result<String, HostError>;

    /// Reads job `id`, waiting up to `wait_ms` for it to finish.
    ///
    /// # Errors
    ///
    /// `Invalid` for an id that is not this plugin's job.
    fn get(&self, who: &Who<'_>, id: &str, wait_ms: u64) -> Result<JobView, HostError>;

    /// Cancels job `id` if it runs and forgets it.
    ///
    /// # Errors
    ///
    /// `Invalid` for an id that is not this plugin's job.
    fn delete(&self, who: &Who<'_>, id: &str) -> Result<(), HostError>;

    /// This node's counts.
    ///
    /// # Errors
    ///
    /// Why the node cannot answer.
    fn stats(&self, who: &Who<'_>) -> Result<JobStats, HostError>;
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&u16::try_from(s.len()).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&u32::try_from(b.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(b);
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8], CodecError> {
    if bytes.len() < n {
        return Err(CodecError::Short);
    }
    let (head, rest) = bytes.split_at(n);
    *bytes = rest;
    Ok(head)
}

fn get_u16(bytes: &mut &[u8]) -> Result<u16, CodecError> {
    let mut a = [0_u8; 2];
    a.copy_from_slice(take(bytes, 2)?);
    Ok(u16::from_le_bytes(a))
}

fn get_u32(bytes: &mut &[u8]) -> Result<u32, CodecError> {
    let mut a = [0_u8; 4];
    a.copy_from_slice(take(bytes, 4)?);
    Ok(u32::from_le_bytes(a))
}

fn get_u64(bytes: &mut &[u8]) -> Result<u64, CodecError> {
    let mut a = [0_u8; 8];
    a.copy_from_slice(take(bytes, 8)?);
    Ok(u64::from_le_bytes(a))
}

fn get_str(bytes: &mut &[u8]) -> Result<String, CodecError> {
    let n = usize::from(get_u16(bytes)?);
    if n > MAX_NAME_BYTES {
        return Err(CodecError::Bound);
    }
    // ALLOC: at most MAX_NAME_BYTES, checked above.
    std::str::from_utf8(take(bytes, n)?)
        .map(str::to_owned)
        .map_err(|_| CodecError::Text)
}

fn get_bytes(bytes: &mut &[u8], max: usize) -> Result<Vec<u8>, CodecError> {
    let n = get_u32(bytes)? as usize;
    if n > max {
        return Err(CodecError::Bound);
    }
    // ALLOC: at most `max`, checked above.
    Ok(take(bytes, n)?.to_vec())
}

fn version(bytes: &mut &[u8]) -> Result<(), CodecError> {
    if take(bytes, 1)?.first().copied() != Some(VERSION) {
        return Err(CodecError::Version);
    }
    Ok(())
}

fn done(bytes: &[u8]) -> Result<(), CodecError> {
    if bytes.is_empty() {
        Ok(())
    } else {
        Err(CodecError::Trailing)
    }
}

/// A `job.submit` call's bytes.
pub fn encode_submit(index: &str, body: &[u8]) -> Vec<u8> {
    // ALLOC: one call's bytes, bounded by the point's ceiling on the host.
    let mut out = vec![VERSION];
    put_str(&mut out, index);
    put_bytes(&mut out, body);
    out
}

/// A `job.submit` call from bytes.
///
/// # Errors
///
/// Bytes that are not a call, or one over a ceiling.
pub fn decode_submit(bytes: &[u8]) -> Result<(String, Vec<u8>), CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let index = get_str(&mut i)?;
    let body = get_bytes(&mut i, MAX_BODY_BYTES)?;
    done(i)?;
    Ok((index, body))
}

/// A `job.get` call's bytes.
pub fn encode_get(id: &str, wait_ms: u64) -> Vec<u8> {
    // ALLOC: one call's bytes, at most MAX_NAME_BYTES and a few more.
    let mut out = vec![VERSION];
    put_str(&mut out, id);
    out.extend_from_slice(&wait_ms.to_le_bytes());
    out
}

/// A `job.get` call from bytes.
///
/// # Errors
///
/// Bytes that are not a call.
pub fn decode_get(bytes: &[u8]) -> Result<(String, u64), CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let id = get_str(&mut i)?;
    let wait = get_u64(&mut i)?;
    done(i)?;
    Ok((id, wait))
}

/// A `job.delete` call's bytes.
pub fn encode_delete(id: &str) -> Vec<u8> {
    // ALLOC: one call's bytes, at most MAX_NAME_BYTES and a few more.
    let mut out = vec![VERSION];
    put_str(&mut out, id);
    out
}

/// A `job.delete` call from bytes.
///
/// # Errors
///
/// Bytes that are not a call.
pub fn decode_delete(bytes: &[u8]) -> Result<String, CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let id = get_str(&mut i)?;
    done(i)?;
    Ok(id)
}

/// A `job.get` answer's bytes.
pub fn encode_view(view: &JobView) -> Vec<u8> {
    // ALLOC: one answer, bounded by the result ceiling the host checks.
    let mut out = vec![VERSION];
    out.push(match view.state {
        JobState::Running => 0,
        JobState::Succeeded => 1,
        JobState::Failed => 2,
    });
    put_bytes(&mut out, &view.body);
    out
}

/// A `job.get` answer from bytes.
///
/// # Errors
///
/// Bytes that are not an answer.
pub fn decode_view(bytes: &[u8]) -> Result<JobView, CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let state = match take(&mut i, 1)?.first().copied() {
        Some(0) => JobState::Running,
        Some(1) => JobState::Succeeded,
        Some(2) => JobState::Failed,
        _ => return Err(CodecError::Value),
    };
    let body = get_bytes(&mut i, MAX_RESULT_BYTES)?;
    done(i)?;
    Ok(JobView { state, body })
}

/// A `job.stats` answer's bytes.
pub fn encode_stats(s: &JobStats) -> Vec<u8> {
    // ALLOC: one answer of 33 bytes.
    let mut out = vec![VERSION];
    for n in [s.running, s.submitted, s.cancelled, s.rejected] {
        out.extend_from_slice(&n.to_le_bytes());
    }
    out
}

/// A `job.stats` answer from bytes.
///
/// # Errors
///
/// Bytes that are not an answer.
pub fn decode_stats(bytes: &[u8]) -> Result<JobStats, CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let s = JobStats {
        running: get_u64(&mut i)?,
        submitted: get_u64(&mut i)?,
        cancelled: get_u64(&mut i)?,
        rejected: get_u64(&mut i)?,
    };
    done(i)?;
    Ok(s)
}

/// Serves the `job.*` operations from a [`JobSink`] and every other operation
/// from `rest`.
pub struct JobBackend {
    sink: Arc<dyn JobSink>,
    rest: Arc<dyn Backend>,
}

impl JobBackend {
    /// A backend running jobs through `sink`, deferring the rest to `rest`.
    pub fn new(sink: Arc<dyn JobSink>, rest: Arc<dyn Backend>) -> Self {
        Self { sink, rest }
    }
}

impl Backend for JobBackend {
    fn call(&self, who: &Who<'_>, op: &str, input: &[u8]) -> Result<Vec<u8>, HostError> {
        let bad = |_| HostError::Invalid;
        match op {
            "job.submit" => {
                let (index, body) = decode_submit(input).map_err(bad)?;
                let id = self.sink.submit(who, &index, &body)?;
                // ALLOC: one id, a few bytes.
                let mut out = vec![VERSION];
                put_str(&mut out, &id);
                Ok(out)
            }
            "job.get" => {
                let (id, wait) = decode_get(input).map_err(bad)?;
                Ok(encode_view(&self.sink.get(who, &id, wait)?))
            }
            "job.delete" => {
                let id = decode_delete(input).map_err(bad)?;
                self.sink.delete(who, &id)?;
                Ok(vec![VERSION])
            }
            "job.stats" => {
                let mut i = input;
                version(&mut i).map_err(bad)?;
                done(i).map_err(bad)?;
                Ok(encode_stats(&self.sink.stats(who)?))
            }
            _ => self.rest.call(who, op, input),
        }
    }
}

/// A `job.submit` answer from bytes: the job's id.
///
/// # Errors
///
/// Bytes that are not an answer.
pub fn decode_id(bytes: &[u8]) -> Result<String, CodecError> {
    let mut i = bytes;
    version(&mut i)?;
    let id = get_str(&mut i)?;
    done(i)?;
    Ok(id)
}
