//! The files of a shard's commit, held apart from the engine's directory so
//! a publish can upload them after the shard's lock is let go.
//!
//! An in-memory directory hands out a file as a reference to its bytes, so
//! taking the snapshot costs a reference count a file, and the bytes of a
//! file the engine deletes meanwhile (a merged-away segment) stay until the
//! publish is done.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Error as IoError, ErrorKind};
use std::sync::Arc;

use lucene_store::{Directory, Error, FsIndexOutput, Input, Lock, Result};

/// The files of one commit, readable and nothing else.
pub(super) struct Snapshot {
    files: BTreeMap<String, Arc<[u8]>>,
    /// The commit's files in the order the engine listed them.
    names: Vec<String>,
}

impl Snapshot {
    /// The `names` of `dir`, as they are now.
    ///
    /// # Errors
    ///
    /// A file that does not open.
    pub(super) fn of(dir: &dyn Directory, names: &[String]) -> std::result::Result<Self, String> {
        // ALLOC: one entry a file of the commit, at most manifest::MAX_FILES.
        let mut files = BTreeMap::new();
        for name in names {
            let bytes = match dir.open(name).map_err(|e| format!("{name}: {e}"))? {
                Input::Shared(bytes) => bytes,
                // ALLOC: only a directory that does not share its bytes copies them; the engine's does.
                other => Arc::from(other.as_ref()),
            };
            files.insert(name.clone(), bytes);
        }
        Ok(Self {
            files,
            // ALLOC: one name a file of the commit, at most manifest::MAX_FILES.
            names: names.to_vec(),
        })
    }

    /// The commit's file names.
    pub(super) fn names(&self) -> &[String] {
        &self.names
    }
}

fn refused(what: &str) -> Error {
    Error::Io(IoError::new(
        ErrorKind::Unsupported,
        format!("a snapshot is read only: {what}"),
    ))
}

impl Directory for Snapshot {
    fn list_all(&self) -> Result<Vec<String>> {
        Ok(self.files.keys().cloned().collect())
    }

    fn open(&self, name: &str) -> Result<Input> {
        self.files
            .get(name)
            .map(|bytes| Input::Shared(Arc::clone(bytes)))
            .ok_or_else(|| Error::Io(IoError::new(ErrorKind::NotFound, name.to_owned())))
    }

    fn file_length(&self, name: &str) -> Result<u64> {
        self.files
            .get(name)
            .map(|bytes| bytes.len() as u64)
            .ok_or_else(|| Error::Io(IoError::new(ErrorKind::NotFound, name.to_owned())))
    }

    fn create_output(&self, _: &str) -> Result<FsIndexOutput> {
        Err(refused("create_output"))
    }

    fn sync(&self, _: &[String]) -> Result<()> {
        Err(refused("sync"))
    }

    fn rename(&self, _: &str, _: &str) -> Result<()> {
        Err(refused("rename"))
    }

    fn delete_file(&self, _: &str) -> Result<()> {
        Err(refused("delete_file"))
    }

    fn sync_meta_data(&self) -> Result<()> {
        Err(refused("sync_meta_data"))
    }

    fn obtain_lock(&self, _: &str) -> Result<Box<dyn Lock>> {
        Err(refused("obtain_lock"))
    }

    fn pending_deletions(&self) -> Result<BTreeSet<String>> {
        Ok(BTreeSet::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucene_store::{ByteBuffersDirectory, DataOutput};

    fn dir_with(files: &[(&str, &[u8])]) -> ByteBuffersDirectory {
        let dir = ByteBuffersDirectory::new();
        for (name, bytes) in files {
            let mut out = dir.create_output(name).expect("creates");
            out.write_bytes(bytes);
            out.close().expect("closes");
        }
        dir
    }

    #[test]
    fn a_snapshot_lists_opens_and_measures_the_files_it_was_taken_of() {
        let dir = dir_with(&[("_0.cfs", b"abc"), ("_1.cfs", b"defgh"), ("other", b"x")]);
        let names = vec!["_0.cfs".to_owned(), "_1.cfs".to_owned()];
        let snapshot = Snapshot::of(&dir, &names).expect("takes");
        assert_eq!(snapshot.names(), names);
        assert_eq!(snapshot.list_all().expect("lists"), names);
        assert_eq!(snapshot.file_length("_1.cfs").expect("length"), 5);
        assert_eq!(snapshot.open("_0.cfs").expect("opens").as_ref(), b"abc");
        assert!(snapshot.open("other").is_err(), "not in the commit");
        assert!(snapshot.file_length("other").is_err());
    }

    #[test]
    fn a_snapshot_keeps_a_file_the_engine_deletes_afterwards() {
        let dir = dir_with(&[("_0.cfs", b"abc")]);
        let snapshot = Snapshot::of(&dir, &["_0.cfs".to_owned()]).expect("takes");
        dir.delete_file("_0.cfs").expect("deletes");
        assert_eq!(snapshot.open("_0.cfs").expect("opens").as_ref(), b"abc");
    }

    #[test]
    fn a_file_the_directory_lacks_fails_the_snapshot() {
        let dir = dir_with(&[]);
        assert!(Snapshot::of(&dir, &["_0.cfs".to_owned()]).is_err());
    }

    #[test]
    fn a_snapshot_is_read_only() {
        let snapshot = Snapshot::of(&dir_with(&[]), &[]).expect("takes");
        assert!(snapshot.create_output("a").is_err());
        assert!(snapshot.sync(&[]).is_err());
        assert!(snapshot.rename("a", "b").is_err());
        assert!(snapshot.delete_file("a").is_err());
        assert!(snapshot.sync_meta_data().is_err());
        assert!(snapshot.obtain_lock("a").is_err());
        assert!(snapshot.pending_deletions().expect("none").is_empty());
    }
}
