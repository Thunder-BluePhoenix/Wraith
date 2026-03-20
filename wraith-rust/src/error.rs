// Re-export anyhow's core types so callers import from one place.
pub use anyhow::{anyhow, bail, Context, Error, Result};

/// Build a "process not found" error with helpful context.
pub fn err_process_not_found(pid: i32) -> Error {
    anyhow!("Process {} not found — check that /proc/{} exists", pid, pid)
}

/// Build a permission-denied error with a root hint.
pub fn err_permission(action: &str, pid: i32) -> Error {
    anyhow!(
        "Permission denied for '{}' on PID {} — try running as root or set /proc/sys/kernel/yama/ptrace_scope to 0",
        action,
        pid
    )
}
