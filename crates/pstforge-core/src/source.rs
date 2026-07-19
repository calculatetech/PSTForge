use std::fs::{File, Metadata, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const HASH_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("source path is a symlink: {0}")]
    Symlink(PathBuf),
    #[error("source is not a regular file: {0}")]
    NotRegular(PathBuf),
    #[error("source identity changed while it was being opened: {0}")]
    OpenRace(PathBuf),
    #[error("source changed during inspection: {0}")]
    Changed(PathBuf),
    #[error("output path contains or aliases the source: {0}")]
    UnsafeOutput(PathBuf),
    #[error("source timestamp cannot be represented as UTC: {0}")]
    InvalidTimestamp(PathBuf),
    #[error("source canonical path is not valid UTF-8: {0:?}")]
    NonUtf8Path(PathBuf),
    #[error("source hashing was interrupted")]
    Interrupted,
    #[error("cannot {operation} source {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub canonical_path: String,
    pub device: u64,
    pub inode: u64,
    pub size_bytes: u64,
    pub modified_at: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StatIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

pub struct SourceFile {
    file: File,
    path: PathBuf,
    stat: StatIdentity,
    identity: SourceIdentity,
}

impl SourceFile {
    pub fn open(path: &Path) -> Result<Self, SourceError> {
        Self::open_with_interrupt(path, None)
    }

    pub(crate) fn open_interruptible(
        path: &Path,
        interrupted: &AtomicBool,
    ) -> Result<Self, SourceError> {
        Self::open_with_interrupt(path, Some(interrupted))
    }

    fn open_with_interrupt(
        path: &Path,
        interrupted: Option<&AtomicBool>,
    ) -> Result<Self, SourceError> {
        let initial = path
            .symlink_metadata()
            .map_err(|source| io("inspect", path, source))?;
        if initial.file_type().is_symlink() {
            return Err(SourceError::Symlink(path.to_path_buf()));
        }
        if !initial.is_file() {
            return Err(SourceError::NotRegular(path.to_path_buf()));
        }

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NOATIME)
            .open(path)
            .map_err(|source| io("open read-only", path, source))?;
        let opened = file
            .metadata()
            .map_err(|source| io("read metadata for", path, source))?;
        if !opened.is_file() {
            return Err(SourceError::NotRegular(path.to_path_buf()));
        }

        let canonical_path = path
            .canonicalize()
            .map_err(|source| io("canonicalize", path, source))?;
        let canonical_metadata = canonical_path
            .metadata()
            .map_err(|source| io("read canonical metadata for", &canonical_path, source))?;
        let stat = StatIdentity::from(&opened);
        if stat != StatIdentity::from(&initial) || stat != StatIdentity::from(&canonical_metadata) {
            return Err(SourceError::OpenRace(path.to_path_buf()));
        }

        let modified_at = timestamp(&canonical_path, stat)?;
        let sha256 = hash_file_at_interruptible(&file, &canonical_path, interrupted)?;
        let after_hash = file
            .metadata()
            .map_err(|source| io("recheck after hashing", &canonical_path, source))?;
        if stat != StatIdentity::from(&after_hash) {
            return Err(SourceError::Changed(canonical_path));
        }
        let canonical_display = canonical_path
            .to_str()
            .ok_or_else(|| SourceError::NonUtf8Path(canonical_path.clone()))?
            .to_owned();
        let identity = SourceIdentity {
            canonical_path: canonical_display,
            device: stat.device,
            inode: stat.inode,
            size_bytes: stat.size,
            modified_at,
            sha256,
        };

        Ok(Self {
            file,
            path: canonical_path,
            stat,
            identity,
        })
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    pub fn identity(&self) -> &SourceIdentity {
        &self.identity
    }

    pub fn verify_unchanged(&self) -> Result<(), SourceError> {
        self.verify_unchanged_observing(|_| {})
    }

    pub(crate) fn verify_unchanged_interruptible(
        &self,
        interrupted: &AtomicBool,
    ) -> Result<(), SourceError> {
        self.verify_unchanged_with_interrupt(Some(interrupted), |_| {})
    }

    fn verify_unchanged_observing(&self, observer: impl FnMut(u64)) -> Result<(), SourceError> {
        self.verify_unchanged_with_interrupt(None, observer)
    }

    fn verify_unchanged_with_interrupt(
        &self,
        interrupted: Option<&AtomicBool>,
        mut observer: impl FnMut(u64),
    ) -> Result<(), SourceError> {
        let fd_metadata = self
            .file
            .metadata()
            .map_err(|source| io("recheck open", &self.path, source))?;
        let path_metadata = self
            .path
            .symlink_metadata()
            .map_err(|source| io("recheck path", &self.path, source))?;
        if path_metadata.file_type().is_symlink()
            || self.stat != StatIdentity::from(&fd_metadata)
            || self.stat != StatIdentity::from(&path_metadata)
        {
            return Err(SourceError::Changed(self.path.clone()));
        }
        observer(self.stat.size);
        if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(SourceError::Interrupted);
        }
        let final_fd_metadata = self
            .file
            .metadata()
            .map_err(|source| io("final recheck of open", &self.path, source))?;
        let final_path_metadata = self
            .path
            .symlink_metadata()
            .map_err(|source| io("final recheck of path", &self.path, source))?;
        if final_path_metadata.file_type().is_symlink()
            || self.stat != StatIdentity::from(&final_fd_metadata)
            || self.stat != StatIdentity::from(&final_path_metadata)
        {
            return Err(SourceError::Changed(self.path.clone()));
        }
        Ok(())
    }
}

pub fn validate_output_relationship(source: &Path, output: &Path) -> Result<(), SourceError> {
    let source = source
        .canonicalize()
        .map_err(|error| io("canonicalize", source, error))?;
    let output = canonicalize_existing_ancestor(output)?;
    let aliases_source = output.metadata().is_ok_and(|output_metadata| {
        source.metadata().is_ok_and(|source_metadata| {
            output_metadata.dev() == source_metadata.dev()
                && output_metadata.ino() == source_metadata.ino()
        })
    });
    if source.starts_with(&output) || output == source || aliases_source {
        return Err(SourceError::UnsafeOutput(output));
    }
    Ok(())
}

fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf, SourceError> {
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        let name = current
            .file_name()
            .ok_or_else(|| SourceError::UnsafeOutput(path.to_path_buf()))?;
        missing.push(name.to_os_string());
        current = current
            .parent()
            .ok_or_else(|| SourceError::UnsafeOutput(path.to_path_buf()))?;
    }
    let mut result = current
        .canonicalize()
        .map_err(|error| io("canonicalize output ancestor", current, error))?;
    for component in missing.iter().rev() {
        result.push(component);
    }
    Ok(result)
}

fn hash_file_at_interruptible(
    file: &File,
    path: &Path,
    interrupted: Option<&AtomicBool>,
) -> Result<String, SourceError> {
    hash_file_at_interruptible_observing(file, path, interrupted, |_| {})
}

fn hash_file_at_interruptible_observing(
    file: &File,
    path: &Path,
    interrupted: Option<&AtomicBool>,
    mut observer: impl FnMut(u64),
) -> Result<String, SourceError> {
    use std::os::unix::fs::FileExt;

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    let mut offset = 0_u64;
    loop {
        if interrupted.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(SourceError::Interrupted);
        }
        let read = file
            .read_at(&mut buffer, offset)
            .map_err(|source| io("hash", path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        offset = offset
            .checked_add(u64::try_from(read).map_err(|_| SourceError::Changed(path.to_path_buf()))?)
            .ok_or_else(|| SourceError::Changed(path.to_path_buf()))?;
        observer(offset);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn timestamp(path: &Path, stat: StatIdentity) -> Result<String, SourceError> {
    let nanos = u32::try_from(stat.modified_nanoseconds)
        .map_err(|_| SourceError::InvalidTimestamp(path.to_path_buf()))?;
    DateTime::<Utc>::from_timestamp(stat.modified_seconds, nanos)
        .map(|value| value.to_rfc3339())
        .ok_or_else(|| SourceError::InvalidTimestamp(path.to_path_buf()))
}

fn io(operation: &'static str, path: &Path, source: std::io::Error) -> SourceError {
    SourceError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

impl From<&Metadata> for StatIdentity {
    fn from(value: &Metadata) -> Self {
        Self {
            device: value.dev(),
            inode: value.ino(),
            size: value.size(),
            modified_seconds: value.mtime(),
            modified_nanoseconds: value.mtime_nsec(),
            changed_seconds: value.ctime(),
            changed_nanoseconds: value.ctime_nsec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::AtomicBool;

    use tempfile::tempdir;

    use super::{SourceError, SourceFile, validate_output_relationship};

    #[test]
    fn hashes_regular_file_and_detects_change() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let path = directory.path().join("mail.pst");
        fs::write(&path, b"pst bytes")?;
        let source = SourceFile::open(&path)?;
        assert_eq!(
            source.identity().sha256,
            "99d7b1f5d1a9ac2aead14c3702f6d0b03eed5119a3e8012f80aad013b7981456"
        );
        fs::write(&path, b"changed bytes")?;
        assert!(matches!(
            source.verify_unchanged(),
            Err(SourceError::Changed(_))
        ));
        Ok(())
    }

    #[test]
    fn source_hashing_honors_an_already_requested_interruption()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let path = directory.path().join("mail.pst");
        fs::write(&path, vec![0_u8; 2 * 1024 * 1024])?;
        let interrupted = AtomicBool::new(true);
        assert!(matches!(
            SourceFile::open_interruptible(&path, &interrupted),
            Err(SourceError::Interrupted)
        ));
        Ok(())
    }

    #[test]
    fn final_identity_check_detects_same_size_change_to_hashed_region()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let path = directory.path().join("mail.pst");
        let original = vec![0_u8; super::HASH_BUFFER_SIZE * 2];
        fs::write(&path, &original)?;
        let source = SourceFile::open(&path)?;
        let mut changed = original;
        changed[0] = 1;
        let mut injected = false;
        let result = source.verify_unchanged_observing(|_| {
            if !injected {
                fs::write(&path, &changed).expect("inject same-size source mutation");
                injected = true;
            }
        });
        assert!(injected);
        assert!(matches!(result, Err(SourceError::Changed(_))));
        Ok(())
    }

    #[test]
    fn refuses_source_symlink() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let target = directory.path().join("target.pst");
        let link = directory.path().join("link.pst");
        fs::write(&target, b"pst")?;
        symlink(&target, &link)?;
        assert!(matches!(
            SourceFile::open(&link),
            Err(SourceError::Symlink(_))
        ));
        Ok(())
    }

    #[test]
    fn refuses_output_that_contains_source() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let source = directory.path().join("source.pst");
        fs::write(&source, b"pst")?;
        assert!(matches!(
            validate_output_relationship(&source, directory.path()),
            Err(SourceError::UnsafeOutput(_))
        ));
        Ok(())
    }

    #[test]
    fn refuses_output_hard_linked_to_source() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempdir()?;
        let source = directory.path().join("source.pst");
        let alias = directory.path().join("alias.pst");
        fs::write(&source, b"pst")?;
        fs::hard_link(&source, &alias)?;
        assert!(matches!(
            validate_output_relationship(&source, &alias),
            Err(SourceError::UnsafeOutput(_))
        ));
        Ok(())
    }
}
