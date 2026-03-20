use crate::error::{anyhow, Context, Result};
use std::fmt;

/// The type of an open file descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FdType {
    /// Regular file on disk — can be reopened by path on destination.
    Regular,
    /// Anonymous pipe — state is lost in v1; logged as a warning.
    Pipe,
    /// Network socket — not preserved in v1 (Phase 8.2).
    Socket,
    /// Device node (/dev/...) — not preserved in v1 (Phase 8.3).
    Device,
    /// Open directory handle.
    Directory,
    /// Anything else (epoll, inotify, timerfd, eventfd, memfd, ...).
    Other(String),
}

impl fmt::Display for FdType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FdType::Regular      => write!(f, "regular"),
            FdType::Pipe         => write!(f, "pipe"),
            FdType::Socket       => write!(f, "socket"),
            FdType::Device       => write!(f, "device"),
            FdType::Directory    => write!(f, "directory"),
            FdType::Other(s)     => write!(f, "other:{}", s),
        }
    }
}

/// An open file descriptor captured from /proc/<pid>/fd + /proc/<pid>/fdinfo.
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    /// The file descriptor number (0 = stdin, 1 = stdout, 2 = stderr, ...).
    pub fd: u32,

    /// The type of resource this fd points to.
    pub fd_type: FdType,

    /// The resolved symlink from /proc/<pid>/fd/<fd>.
    /// For regular files: absolute path. For sockets/pipes: "socket:[inode]".
    pub path: String,

    /// Current seek offset (from /proc/<pid>/fdinfo/<fd>, field "pos").
    pub offset: u64,

    /// Open flags (from /proc/<pid>/fdinfo/<fd>, field "flags", octal).
    pub flags: i32,
}

impl FileDescriptor {
    /// True if this fd can be restored on the destination (regular file, same path).
    pub fn is_restorable(&self) -> bool {
        self.fd_type == FdType::Regular
    }
}

/// Enumerate all open file descriptors for a process.
///
/// Reads /proc/<pid>/fd (symlinks) and /proc/<pid>/fdinfo/<fd> (offset + flags).
/// The process does **not** need to be ptrace-stopped for this — the kernel
/// snapshots fd state atomically per-fd.
pub fn enumerate_fds(pid: i32) -> Result<Vec<FileDescriptor>> {
    let fd_dir = format!("/proc/{}/fd", pid);
    let entries = std::fs::read_dir(&fd_dir)
        .with_context(|| format!("Cannot read {}", fd_dir))?;

    let mut fds = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e)  => e,
            Err(e) => {
                log::warn!("Skipping unreadable fd entry: {}", e);
                continue;
            }
        };

        let fd_num: u32 = match entry
            .file_name()
            .to_string_lossy()
            .parse()
        {
            Ok(n)  => n,
            Err(_) => {
                log::warn!("Skipping non-numeric fd entry: {:?}", entry.file_name());
                continue;
            }
        };

        // Resolve the symlink to get the target path.
        let target = match std::fs::read_link(entry.path()) {
            Ok(t)  => t.to_string_lossy().to_string(),
            Err(e) => {
                log::warn!("Cannot read symlink for fd {}: {}", fd_num, e);
                continue;
            }
        };

        let fd_type = classify_fd(&target);

        // Read offset and flags from fdinfo.
        let (offset, flags) = read_fdinfo(pid, fd_num).unwrap_or_else(|e| {
            log::warn!("Cannot read fdinfo for fd {}: {}", fd_num, e);
            (0, 0)
        });

        fds.push(FileDescriptor {
            fd: fd_num,
            fd_type,
            path: target,
            offset,
            flags,
        });
    }

    // Sort by fd number for deterministic output.
    fds.sort_by_key(|f| f.fd);

    log::debug!("Enumerated {} file descriptors for PID {}", fds.len(), pid);
    Ok(fds)
}

/// Classify an fd based on its /proc/pid/fd/<n> symlink target.
fn classify_fd(target: &str) -> FdType {
    if target.starts_with("socket:") {
        FdType::Socket
    } else if target.starts_with("pipe:") {
        FdType::Pipe
    } else if target.starts_with('/') {
        // Check whether it's a device node.
        if target.starts_with("/dev/") {
            FdType::Device
        } else if std::fs::metadata(target)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            FdType::Directory
        } else {
            FdType::Regular
        }
    } else {
        FdType::Other(target.to_string())
    }
}

/// Read the current file offset and open flags from /proc/<pid>/fdinfo/<fd>.
///
/// Format:
///   pos:    4096
///   flags:  0100002   ← octal open(2) flags
///   mnt_id: 25
fn read_fdinfo(pid: i32, fd: u32) -> Result<(u64, i32)> {
    let path = format!("/proc/{}/fdinfo/{}", pid, fd);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path))?;

    let mut offset = 0u64;
    let mut flags  = 0i32;

    for line in content.lines() {
        if let Some(val) = line.strip_prefix("pos:\t").or_else(|| line.strip_prefix("pos:")) {
            offset = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("flags:\t").or_else(|| line.strip_prefix("flags:")) {
            // Flags are printed in octal by the kernel.
            flags = i32::from_str_radix(val.trim(), 8).unwrap_or(0);
        }
    }

    Ok((offset, flags))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_socket() {
        assert_eq!(classify_fd("socket:[12345]"), FdType::Socket);
    }

    #[test]
    fn test_classify_pipe() {
        assert_eq!(classify_fd("pipe:[99]"), FdType::Pipe);
    }

    #[test]
    fn test_classify_regular() {
        assert_eq!(classify_fd("/tmp/some_file.txt"), FdType::Regular);
    }

    #[test]
    fn test_classify_device() {
        assert_eq!(classify_fd("/dev/null"), FdType::Device);
    }

    #[test]
    fn test_fd_display() {
        assert_eq!(FdType::Socket.to_string(),      "socket");
        assert_eq!(FdType::Pipe.to_string(),        "pipe");
        assert_eq!(FdType::Regular.to_string(),     "regular");
        assert_eq!(FdType::Device.to_string(),      "device");
        assert_eq!(FdType::Directory.to_string(),   "directory");
        assert_eq!(FdType::Other("memfd".into()).to_string(), "other:memfd");
    }

    #[test]
    fn test_is_restorable_only_regular() {
        let make = |t: FdType| FileDescriptor { fd: 0, fd_type: t, path: String::new(), offset: 0, flags: 0 };
        assert!( make(FdType::Regular).is_restorable());
        assert!(!make(FdType::Socket).is_restorable());
        assert!(!make(FdType::Pipe).is_restorable());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_enumerate_self_fds() {
        // enumerate fds for the current process — always has at least stdin/stdout/stderr
        let fds = enumerate_fds(std::process::id() as i32).expect("enumerate_fds");
        assert!(fds.len() >= 3, "Expected at least stdin/stdout/stderr, got {}", fds.len());
        // fds should be sorted
        let nums: Vec<u32> = fds.iter().map(|f| f.fd).collect();
        let mut sorted = nums.clone();
        sorted.sort();
        assert_eq!(nums, sorted);
    }
}
