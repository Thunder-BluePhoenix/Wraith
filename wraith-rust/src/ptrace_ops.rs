use crate::error::{anyhow, err_permission, err_process_not_found, Result};
use crate::registers::Registers;
use crate::utils;
use nix::errno::Errno;
use nix::sys::ptrace;
use nix::sys::wait::waitpid;
use nix::unistd::Pid;

/// RAII guard that holds a ptrace attachment to a process.
///
/// ## Lifecycle
///
/// 1. `ProcessLock::attach(pid)` — sends SIGSTOP, waits until the process stops.
/// 2. Caller performs reads (registers, memory) while process is frozen.
/// 3. `detach()` or `Drop` — sends PTRACE_DETACH, process resumes.
///
/// The process is **never killed** by this type. If the caller panics or returns
/// an error, `Drop` resumes the process automatically. This is the safety
/// guarantee that makes rollback possible: the source process stays frozen
/// until the Python orchestrator explicitly calls `--resume`.
pub struct ProcessLock {
    pid:      Pid,
    detached: bool,
}

impl ProcessLock {
    /// Attach to a running process and freeze it.
    ///
    /// Fails if the process does not exist, if permissions are insufficient,
    /// or if the kernel rejects the attach for any other reason.
    pub fn attach(raw_pid: i32) -> Result<Self> {
        if !utils::pid_exists(raw_pid) {
            return Err(err_process_not_found(raw_pid));
        }

        let pid = Pid::from_raw(raw_pid);

        ptrace::attach(pid).map_err(|e| {
            if e == Errno::EPERM || e == Errno::EACCES {
                err_permission("ptrace attach", raw_pid)
            } else {
                anyhow!("ptrace::attach({}) failed: {}", raw_pid, e)
            }
        })?;

        // Block until the kernel delivers SIGSTOP to the target.
        waitpid(pid, None)
            .map_err(|e| anyhow!("waitpid({}) after attach failed: {}", raw_pid, e))?;

        log::debug!("Attached and froze PID {}", raw_pid);

        Ok(ProcessLock { pid, detached: false })
    }

    /// Detach from the process, resuming its execution.
    ///
    /// Safe to call multiple times — subsequent calls are no-ops.
    pub fn detach(&mut self) -> Result<()> {
        if !self.detached {
            ptrace::detach(self.pid, None)
                .map_err(|e| anyhow!("ptrace::detach({}) failed: {}", self.pid, e))?;
            self.detached = true;
            log::debug!("Detached and resumed PID {}", self.pid);
        }
        Ok(())
    }

    /// Capture all register state from the frozen process.
    pub fn capture_registers(&self) -> Result<Registers> {
        let regs = Registers::from_ptrace(self.pid)?;
        regs.validate()?;
        Ok(regs)
    }

    /// The PID of the attached process.
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Whether the lock has already been released.
    pub fn is_detached(&self) -> bool {
        self.detached
    }
}

impl Drop for ProcessLock {
    /// Ensure the process is always resumed, even on panic.
    fn drop(&mut self) {
        if !self.detached {
            if let Err(e) = ptrace::detach(self.pid, None) {
                // Log but do not panic — Drop must not unwind.
                log::warn!(
                    "Failed to detach from PID {} during drop: {} — process may be stuck in T state",
                    self.pid,
                    e
                );
            } else {
                log::debug!("Drop: detached PID {}", self.pid);
            }
        }
    }
}
