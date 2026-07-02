use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;

const INSTANCE_LOCK_ENV: &str = "AZAD_INSTANCE_LOCK_PATH";

pub struct SingleInstanceGuard {
  _lock_file: File,
  _lock_path: PathBuf,
}

#[derive(Debug)]
pub enum SingleInstanceError {
  AlreadyRunning,
  HomeUnavailable,
  LockIo { path: PathBuf, source: io::Error },
}

impl fmt::Display for SingleInstanceError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::AlreadyRunning => write!(f, "another Azad instance is already running"),
      Self::HomeUnavailable => write!(f, "HOME is not set"),
      Self::LockIo { path, source } => {
        write!(f, "lock I/O failed at {}: {}", path.display(), source)
      }
    }
  }
}

impl std::error::Error for SingleInstanceError {}

pub fn acquire_primary_instance_lock() -> Result<SingleInstanceGuard, SingleInstanceError> {
  let lock_path = lock_path()?;
  if let Some(parent) = lock_path.parent() {
    fs::create_dir_all(parent)
      .map_err(|source| SingleInstanceError::LockIo { path: parent.to_path_buf(), source })?;
  }

  let lock_file = OpenOptions::new()
    .create(true)
    .truncate(false)
    .read(true)
    .write(true)
    .open(&lock_path)
    .map_err(|source| SingleInstanceError::LockIo { path: lock_path.clone(), source })?;

  let fd = lock_file.as_raw_fd();
  let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
  if rc != 0 {
    let source = io::Error::last_os_error();
    if is_lock_contention(&source) {
      return Err(SingleInstanceError::AlreadyRunning);
    }
    return Err(SingleInstanceError::LockIo { path: lock_path, source });
  }

  Ok(SingleInstanceGuard { _lock_file: lock_file, _lock_path: lock_path })
}

fn lock_path() -> Result<PathBuf, SingleInstanceError> {
  if let Some(custom) = std::env::var_os(INSTANCE_LOCK_ENV) {
    return Ok(PathBuf::from(custom));
  }

  let home = std::env::var_os("HOME").ok_or(SingleInstanceError::HomeUnavailable)?;
  Ok(default_lock_path_from_home(home.as_os_str()))
}

fn default_lock_path_from_home(home: &std::ffi::OsStr) -> PathBuf {
  let mut path = PathBuf::from(home);
  path.push("Library");
  path.push("Application Support");
  path.push("Azad");
  path.push("instance.lock");
  path
}

fn is_lock_contention(err: &io::Error) -> bool {
  matches!(
      err.raw_os_error(),
      Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
  )
}

#[cfg(test)]
mod tests {
  use super::{default_lock_path_from_home, is_lock_contention};
  use std::ffi::OsStr;
  use std::io;
  use std::path::PathBuf;

  #[test]
  fn default_lock_path_uses_application_support_dir() {
    let path = default_lock_path_from_home(OsStr::new("/Users/spence"));
    assert_eq!(path, PathBuf::from("/Users/spence/Library/Application Support/Azad/instance.lock"));
  }

  #[test]
  fn lock_contention_detects_would_block() {
    assert!(is_lock_contention(&io::Error::from_raw_os_error(libc::EWOULDBLOCK)));
  }

  #[test]
  fn lock_contention_detects_eagain() {
    assert!(is_lock_contention(&io::Error::from_raw_os_error(libc::EAGAIN)));
  }

  #[test]
  fn lock_contention_ignores_non_contention_errors() {
    assert!(!is_lock_contention(&io::Error::from_raw_os_error(libc::EPERM)));
  }
}
