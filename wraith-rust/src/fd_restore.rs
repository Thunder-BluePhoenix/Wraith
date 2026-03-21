// File descriptor restoration for Phase 4.
//
// # v1 Scope
//
// Full FD restoration (pipes, sockets, devices) requires co-operation from
// inside the target address space — typically via a Unix socket or shared
// memory handshake that the trampoline sets up. That work is planned for
// Phase 8.2 (socket-live-migration).
//
// For v1, we handle regular files only:
//   - Reopen at the same path.
//   - Seek to the captured offset.
//   - Log a warning and skip everything else (pipe, socket, device, directory).
//
// The Python orchestrator (Phase 5) checks `RestoreReport::skipped_fds` and
// can surface warnings to the operator before deciding whether to proceed.

use crate::error::{anyhow, Result};
use crate::proto::wraith::{file_descriptor::FdType, FileDescriptor};
use std::os::unix::fs::OpenOptionsExt;

/// Outcome of attempting to restore a single FD.
#[derive(Debug)]
pub enum FdOutcome {
    /// File opened and seeked successfully; raw OS fd for dup2 injection.
    Restored { fd_num: u32, os_fd: i32 },
    /// Skipped with a human-readable reason (pipes, sockets, etc.).
    Skipped { fd_num: u32, reason: String },
    /// Open or seek failed.
    Failed { fd_num: u32, error: String },
}

impl FdOutcome {
    pub fn fd_num(&self) -> u32 {
        match self {
            FdOutcome::Restored { fd_num, .. }
            | FdOutcome::Skipped  { fd_num, .. }
            | FdOutcome::Failed   { fd_num, .. } => *fd_num,
        }
    }

    pub fn is_restored(&self) -> bool {
        matches!(self, FdOutcome::Restored { .. })
    }
}

/// Summary returned to the caller after attempting all FDs.
#[derive(Debug, Default)]
pub struct RestoreReport {
    pub restored: Vec<FdOutcome>,
    pub skipped:  Vec<FdOutcome>,
    pub failed:   Vec<FdOutcome>,
}

impl RestoreReport {
    pub fn add(&mut self, outcome: FdOutcome) {
        match &outcome {
            FdOutcome::Restored { .. } => self.restored.push(outcome),
            FdOutcome::Skipped  { .. } => self.skipped.push(outcome),
            FdOutcome::Failed   { .. } => self.failed.push(outcome),
        }
    }

    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }
}

/// Handles file descriptor restoration for a snapshot.
///
/// ## Usage
///
/// ```rust,ignore
/// let report = FdRestorer::restore_all(&snapshot.file_descriptors);
/// for outcome in &report.skipped {
///     log::warn!("FD {} skipped: see report", outcome.fd_num());
/// }
/// ```
pub struct FdRestorer;

impl FdRestorer {
    /// Attempt to restore every FD in the list.
    ///
    /// Never panics; all errors are recorded in the report.
    pub fn restore_all(fds: &[FileDescriptor]) -> RestoreReport {
        let mut report = RestoreReport::default();

        for fd_spec in fds {
            let outcome = Self::restore_one(fd_spec);
            log_outcome(&outcome);
            report.add(outcome);
        }

        report
    }

    fn restore_one(fd_spec: &FileDescriptor) -> FdOutcome {
        let fd_num = fd_spec.fd_num;

        // Proto encodes FdType as i32; 0 = Regular, 1 = Pipe, 2 = Socket,
        // 3 = Device, 4 = Directory, 5 = Other.
        let fd_type = FdType::try_from(fd_spec.fd_type).unwrap_or(FdType::Other);

        match fd_type {
            FdType::Regular => Self::restore_regular(fd_num, fd_spec),

            FdType::Pipe => FdOutcome::Skipped {
                fd_num,
                reason: format!(
                    "pipe FDs cannot be restored across machines in v1 \
                     (path: {:?}) — app must reopen",
                    fd_spec.path
                ),
            },

            FdType::Socket => FdOutcome::Skipped {
                fd_num,
                reason: format!(
                    "socket FDs not restored in v1 (path: {:?}) — \
                     planned for Phase 8.2",
                    fd_spec.path
                ),
            },

            FdType::Device => FdOutcome::Skipped {
                fd_num,
                reason: format!(
                    "device FD {} ({:?}) not restored — device may not exist on destination",
                    fd_num, fd_spec.path
                ),
            },

            FdType::Directory => Self::restore_directory(fd_num, fd_spec),

            FdType::Other => FdOutcome::Skipped {
                fd_num,
                reason: format!("unknown FD type for FD {} — skipping", fd_num),
            },
        }
    }

    /// Open a regular file at its captured path and seek to the captured offset.
    fn restore_regular(fd_num: u32, fd_spec: &FileDescriptor) -> FdOutcome {
        if fd_spec.path.is_empty() {
            return FdOutcome::Skipped {
                fd_num,
                reason: "regular file FD has no path (deleted or anonymous)".to_string(),
            };
        }

        // Reconstruct the open flags from the captured flags bitmask.
        let access_mode = fd_spec.open_flags & libc::O_ACCMODE;
        let write_access = access_mode == libc::O_WRONLY || access_mode == libc::O_RDWR;

        let result: Result<i32> = (|| {
            let mut opts = std::fs::OpenOptions::new();
            opts.read(true);
            if write_access {
                opts.write(true);
            }
            // Preserve O_APPEND if it was set.
            if fd_spec.open_flags & libc::O_APPEND != 0 {
                opts.append(true);
            }
            // Preserve O_CLOEXEC if it was set.
            opts.custom_flags(fd_spec.open_flags & libc::O_CLOEXEC);

            let file = opts
                .open(&fd_spec.path)
                .map_err(|e| anyhow!("open {:?}: {}", fd_spec.path, e))?;

            // Seek to the captured file offset.
            use std::io::Seek;
            let mut file = file;
            file.seek(std::io::SeekFrom::Start(fd_spec.file_offset))
                .map_err(|e| anyhow!("seek to {} in {:?}: {}", fd_spec.file_offset, fd_spec.path, e))?;

            // Extract the raw fd without closing the file on drop.
            use std::os::unix::io::IntoRawFd;
            Ok(file.into_raw_fd())
        })();

        match result {
            Ok(os_fd) => FdOutcome::Restored { fd_num, os_fd },
            Err(e) => FdOutcome::Failed {
                fd_num,
                error: format!("{:#}", e),
            },
        }
    }

    /// Open a directory (for O_PATH or fchdir use).
    fn restore_directory(fd_num: u32, fd_spec: &FileDescriptor) -> FdOutcome {
        if fd_spec.path.is_empty() {
            return FdOutcome::Skipped {
                fd_num,
                reason: "directory FD has no path".to_string(),
            };
        }

        let result: Result<i32> = (|| {
            // Open with O_DIRECTORY | O_RDONLY to get a directory fd.
            let flags = libc::O_DIRECTORY | libc::O_RDONLY | libc::O_CLOEXEC;
            let c_path = std::ffi::CString::new(fd_spec.path.as_bytes())
                .map_err(|e| anyhow!("path contains null byte: {}", e))?;

            let os_fd = unsafe { libc::open(c_path.as_ptr(), flags) };
            if os_fd < 0 {
                return Err(anyhow!(
                    "open directory {:?}: {}",
                    fd_spec.path,
                    std::io::Error::last_os_error()
                ));
            }
            Ok(os_fd)
        })();

        match result {
            Ok(os_fd) => FdOutcome::Restored { fd_num, os_fd },
            Err(e) => FdOutcome::Failed {
                fd_num,
                error: format!("{:#}", e),
            },
        }
    }
}

fn log_outcome(outcome: &FdOutcome) {
    match outcome {
        FdOutcome::Restored { fd_num, .. } => {
            log::debug!("FD {}: restored", fd_num);
        }
        FdOutcome::Skipped { fd_num, reason } => {
            log::warn!("FD {}: skipped — {}", fd_num, reason);
        }
        FdOutcome::Failed { fd_num, error } => {
            log::error!("FD {}: failed — {}", fd_num, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::wraith::FileDescriptor;

    fn make_fd(fd_num: u32, fd_type: i32, path: &str, offset: u64) -> FileDescriptor {
        FileDescriptor {
            fd_num,
            fd_type,
            path:        path.to_string(),
            file_offset: offset,
            open_flags:  libc::O_RDONLY,
        }
    }

    #[test]
    fn test_pipe_is_skipped() {
        let fd = make_fd(3, FdType::Pipe as i32, "", 0);
        let outcome = FdRestorer::restore_one(&fd);
        assert!(matches!(outcome, FdOutcome::Skipped { .. }));
    }

    #[test]
    fn test_socket_is_skipped() {
        let fd = make_fd(4, FdType::Socket as i32, "socket:[12345]", 0);
        let outcome = FdRestorer::restore_one(&fd);
        assert!(matches!(outcome, FdOutcome::Skipped { .. }));
    }

    #[test]
    fn test_regular_nonexistent_is_failed() {
        let fd = make_fd(5, FdType::Regular as i32, "/nonexistent/path/wraith_test_xyz", 0);
        let outcome = FdRestorer::restore_one(&fd);
        assert!(matches!(outcome, FdOutcome::Failed { .. }));
    }

    #[test]
    fn test_regular_empty_path_is_skipped() {
        let fd = make_fd(6, FdType::Regular as i32, "", 0);
        let outcome = FdRestorer::restore_one(&fd);
        assert!(matches!(outcome, FdOutcome::Skipped { .. }));
    }

    #[test]
    fn test_restore_report_aggregation() {
        let fds = vec![
            make_fd(3, FdType::Pipe   as i32, "",              0),
            make_fd(4, FdType::Socket as i32, "socket:[1]",    0),
            make_fd(5, FdType::Regular as i32, "/no/such/file", 0),
        ];
        let report = FdRestorer::restore_all(&fds);
        assert_eq!(report.skipped.len(), 2);
        assert_eq!(report.failed.len(), 1);
        assert!(report.has_failures());
    }
}
