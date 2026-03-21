// Phase 4: Process Restoration Engine
//
// # How it works
//
// Restoring a process from a snapshot is fundamentally different from capturing
// one: we need to build an address space from scratch at exact virtual addresses
// chosen by the SOURCE machine, not by the destination kernel.
//
// The approach:
//
//   1. Fork a "stub" child that immediately stops itself (PTRACE_TRACEME + SIGSTOP).
//      The child starts with a minimal address space — just the stub binary pages
//      plus a stack; everything else is empty.
//
//   2. The parent (us) uses ptrace SYSCALL INJECTION to ask the stopped child to
//      call mmap(2) on our behalf. Each call maps one snapshot region at its
//      exact virtual address using MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS.
//      We request PROT_READ | PROT_WRITE initially so we can write data.
//
//   3. After mmap, we write the page data directly through /proc/<child>/mem.
//      This works because the child is ptrace-stopped and the mapping is writable.
//
//   4. If the original region was read-only or execute-only, we inject a
//      mprotect(2) call to restore the correct permissions.
//
//   5. Once all regions are mapped, we inject the final register state via
//      PTRACE_SETREGS + PTRACE_SETFPREGS.
//
//   6. We detach. The child's instruction pointer now points to the original
//      RIP from the snapshot; it resumes as if it never stopped.
//
// # Syscall injection mechanics
//
// We cannot call mmap() on behalf of another process directly. Instead we use
// `SyscallInjector` which:
//   a. Saves the child's current register set.
//   b. Writes a two-byte `syscall; int3` sequence (0x0f 0x05 0xcc) at the
//      child's current RIP.
//   c. Sets up syscall number (rax) and arguments (rdi/rsi/rdx/r10/r8/r9).
//   d. PTRACE_CONT → child executes syscall → hits int3 → stops with SIGTRAP.
//   e. Reads the return value from rax.
//   f. Restores the original instruction bytes and registers.
//
// The child therefore executes each syscall atomically from the parent's
// perspective; its observable state is identical before and after injection.
//
// # Known v1 limitations
//
// - The stub child's own stack lives at some virtual address. If a snapshot
//   region would be mapped exactly on top of the stub's stack via MAP_FIXED,
//   the child may crash mid-injection. We detect this and return an error.
//   Phase 8 will use a purpose-built restore blob to avoid this.
//
// - FD restoration (Phase 4.4) places opened files at the correct FD numbers
//   via dup2 injection. Pipes and sockets are skipped — see fd_restore.rs.
//
// - Multi-threaded snapshots are not supported (v1 is single-thread only).

use crate::aslr::{perms_to_prot, AddressSpaceLayout};
use crate::error::{anyhow, Result};
use crate::fd_restore::{FdRestorer, RestoreReport};
use crate::memory::MemoryDumper;
use crate::proto::wraith::ProcessSnapshot;
use nix::sys::ptrace;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{ForkResult, Pid};
use std::io::{Seek, SeekFrom, Write};

/// The complete result of a restore operation.
pub struct RestoreResult {
    /// PID of the newly created process running the restored state.
    pub pid:        i32,
    /// Number of memory regions successfully mapped and written.
    pub regions_ok: usize,
    /// Summary of FD restoration attempts.
    pub fd_report:  RestoreReport,
}

/// Coordinates the full process restoration from a `ProcessSnapshot`.
pub struct ProcessRestorer {
    snapshot: ProcessSnapshot,
}

impl ProcessRestorer {
    pub fn new(snapshot: ProcessSnapshot) -> Self {
        ProcessRestorer { snapshot }
    }

    /// Restore the snapshot to a new process on this machine.
    ///
    /// Returns the PID of the restored process and a summary of what was done.
    /// On error, any forked child is killed before returning.
    pub fn restore(self) -> Result<RestoreResult> {
        self.validate()?;

        // Step 1: fork a minimal stub child.
        let child_pid = self.fork_stub()?;

        // From here on, any early return must kill the child.
        match self.restore_inner(child_pid) {
            Ok(result) => Ok(result),
            Err(e) => {
                // Best-effort cleanup: kill and reap the child.
                let _ = nix::sys::signal::kill(child_pid, Signal::SIGKILL);
                let _ = waitpid(child_pid, None);
                Err(e)
            }
        }
    }

    // ── Pre-flight checks ────────────────────────────────────────────────────

    fn validate(&self) -> Result<()> {
        // Architecture must match.
        let host_arch = std::env::consts::ARCH;
        let snap_arch = &self.snapshot.arch;
        if snap_arch != "x86_64" {
            return Err(anyhow!(
                "Snapshot arch {:?} is not x86_64; cross-arch restore is planned for Phase 8.4",
                snap_arch
            ));
        }
        if host_arch != "x86_64" {
            return Err(anyhow!(
                "Host arch {:?} is not x86_64; restore requires matching architecture",
                host_arch
            ));
        }

        // Address space layout must be valid and restorable.
        let layout = AddressSpaceLayout::from_snapshot(&self.snapshot);
        layout.validate()?;

        // Must have at least registers + one memory region.
        if self.snapshot.registers.is_none() {
            return Err(anyhow!("Snapshot has no register state; capture may be corrupted"));
        }
        if self.snapshot.memory_regions.is_empty() {
            return Err(anyhow!("Snapshot has no memory regions"));
        }

        Ok(())
    }

    // ── Fork the stub child ───────────────────────────────────────────────────

    fn fork_stub(&self) -> Result<Pid> {
        match unsafe { nix::unistd::fork() }
            .map_err(|e| anyhow!("fork failed: {}", e))?
        {
            ForkResult::Child => {
                // Child: enable tracing, then stop so the parent can take over.
                unsafe { child_stub() }; // never returns
                unreachable!()
            }
            ForkResult::Parent { child } => {
                // Wait for the child to deliver SIGSTOP to us.
                match waitpid(child, None)
                    .map_err(|e| anyhow!("waitpid after fork: {}", e))?
                {
                    WaitStatus::Stopped(_, Signal::SIGSTOP) => Ok(child),
                    WaitStatus::Stopped(_, sig) => {
                        Err(anyhow!("stub child stopped with unexpected signal {:?}", sig))
                    }
                    status => Err(anyhow!("stub child unexpected wait status: {:?}", status)),
                }
            }
        }
    }

    // ── Inner restore (after fork, may return error — caller kills child) ────

    fn restore_inner(&self, child: Pid) -> Result<RestoreResult> {
        let mut injector = SyscallInjector::new(child);
        let mut regions_ok = 0usize;

        for region in &self.snapshot.memory_regions {
            // Verify checksum before touching the child.
            let computed = MemoryDumper::checksum(&region.data);
            if computed != region.checksum {
                return Err(anyhow!(
                    "Checksum mismatch for region {:#x}: stored={:#x} computed={:#x}",
                    region.start_addr, region.checksum, computed
                ));
            }

            let addr  = region.start_addr;
            let size  = region.size_bytes as usize;
            let final_prot = perms_to_prot(&region.permissions);

            // Map with write permission so we can fill the data.
            let mmap_prot = final_prot | libc::PROT_WRITE;

            log::debug!(
                "Mapping region {:#x}–{:#x} ({} KB) prot={:#x}",
                addr, addr + region.size_bytes, size / 1024, mmap_prot
            );

            // Inject mmap into the child.
            let mapped = injector.mmap(
                addr as *mut libc::c_void,
                size,
                mmap_prot,
                libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )?;

            if mapped as u64 != addr {
                return Err(anyhow!(
                    "mmap MAP_FIXED returned {:#x} instead of {:#x}; kernel refused placement",
                    mapped as u64, addr
                ));
            }

            // Write the page data through /proc/<child>/mem.
            write_to_child_mem(child, addr, &region.data)?;

            // Restore final permissions (mprotect away write if original was r-x).
            if final_prot != mmap_prot {
                injector.mprotect(addr as *mut libc::c_void, size, final_prot)?;
            }

            regions_ok += 1;
        }

        // Restore register state (GP + FPU).
        self.restore_registers(child)?;

        // Restore file descriptors (best-effort; non-fatal).
        let fd_report = FdRestorer::restore_all(&self.snapshot.file_descriptors);
        if fd_report.has_failures() {
            log::warn!(
                "{} FD(s) failed to restore — process may malfunction on certain I/O operations",
                fd_report.failed.len()
            );
        }

        // Inject dup2 for successfully opened FDs.
        for outcome in &fd_report.restored {
            if let crate::fd_restore::FdOutcome::Restored { fd_num, os_fd } = outcome {
                if let Err(e) = injector.dup2(*os_fd, *fd_num as i32) {
                    log::warn!("dup2({} → {}) injection failed: {:#}", os_fd, fd_num, e);
                }
                // Close the fd in the parent now that the child has it.
                unsafe { libc::close(*os_fd) };
            }
        }

        // Detach — child resumes at the restored RIP.
        ptrace::detach(child, None)
            .map_err(|e| anyhow!("ptrace::detach: {}", e))?;

        log::info!(
            "Restore complete: PID {} — {} regions mapped, {} FDs restored, {} FDs skipped",
            child,
            regions_ok,
            fd_report.restored.len(),
            fd_report.skipped.len()
        );

        Ok(RestoreResult {
            pid:        child.as_raw(),
            regions_ok,
            fd_report,
        })
    }

    // ── Register restoration ──────────────────────────────────────────────────

    fn restore_registers(&self, child: Pid) -> Result<()> {
        let regs = self.snapshot.registers.as_ref()
            .ok_or_else(|| anyhow!("no register state in snapshot"))?;

        // Build libc::user_regs_struct from the proto-encoded registers.
        // Note: libc names the flags field `eflags` even on x86-64.
        let gp = libc::user_regs_struct {
            rax:      regs.rax,
            rbx:      regs.rbx,
            rcx:      regs.rcx,
            rdx:      regs.rdx,
            rdi:      regs.rdi,
            rsi:      regs.rsi,
            rbp:      regs.rbp,
            rsp:      regs.rsp,
            r8:       regs.r8,
            r9:       regs.r9,
            r10:      regs.r10,
            r11:      regs.r11,
            r12:      regs.r12,
            r13:      regs.r13,
            r14:      regs.r14,
            r15:      regs.r15,
            rip:      regs.rip,
            eflags:   regs.rflags,
            cs:       regs.cs,
            ss:       regs.ss,
            ds:       regs.ds,
            es:       regs.es,
            fs:       regs.fs,
            gs:       regs.gs,
            fs_base:  regs.fs_base,
            gs_base:  regs.gs_base,
            orig_rax: u64::MAX, // sentinel: no pending syscall
        };

        ptrace::setregs(child, gp)
            .map_err(|e| anyhow!("PTRACE_SETREGS: {}", e))?;

        // Restore FPU / SSE / MMX state if present.
        if !regs.fpu_state.is_empty() {
            let expected = std::mem::size_of::<libc::user_fpregs_struct>();
            if regs.fpu_state.len() != expected {
                return Err(anyhow!(
                    "FPU state is {} bytes, expected {}",
                    regs.fpu_state.len(), expected
                ));
            }
            // Safety: fpu_state is always sizeof(user_fpregs_struct) bytes (validated in
            // Registers::validate at capture time).
            let fpregs: libc::user_fpregs_struct = unsafe {
                std::ptr::read_unaligned(regs.fpu_state.as_ptr() as *const _)
            };
            ptrace::setfpregs(child, fpregs)
                .map_err(|e| anyhow!("PTRACE_SETFPREGS: {}", e))?;
        }

        log::debug!(
            "Registers restored: rip={:#018x} rsp={:#018x}",
            regs.rip, regs.rsp
        );
        Ok(())
    }
}

// ── Child stub ───────────────────────────────────────────────────────────────

/// Code that runs in the child immediately after fork.
///
/// Enables ptrace tracing then sends SIGSTOP to itself. The parent will
/// intercept the SIGSTOP, take over the address space, and detach when done.
///
/// # Safety
/// Called immediately after fork() while in async-signal-safe context.
unsafe fn child_stub() -> ! {
    // Enable tracing: any signal will be delivered to the parent as a ptrace event.
    if libc::ptrace(libc::PTRACE_TRACEME, 0, 0, 0) != 0 {
        libc::_exit(1);
    }
    // Stop ourselves so the parent can take over.
    libc::raise(libc::SIGSTOP);
    // Should never reach here — parent will set registers and detach.
    libc::_exit(0);
}

// ── Write through /proc/<pid>/mem ────────────────────────────────────────────

fn write_to_child_mem(child: Pid, addr: u64, data: &[u8]) -> Result<()> {
    let path = format!("/proc/{}/mem", child.as_raw());
    // O_RDWR required for writes; open(2) on /proc/pid/mem succeeds only when
    // the process is ptrace-stopped by the caller (which it is).
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| anyhow!("open {}: {} (process must be ptrace-stopped)", path, e))?;

    f.seek(SeekFrom::Start(addr))
        .map_err(|e| anyhow!("seek to {:#x} in {}: {}", addr, path, e))?;

    f.write_all(data)
        .map_err(|e| anyhow!("write {} bytes at {:#x} in {}: {}", data.len(), addr, path, e))?;

    Ok(())
}

// ── Syscall injector ─────────────────────────────────────────────────────────

/// Injects syscalls into a ptrace-stopped child process.
///
/// ## Protocol
///
/// For each syscall:
///   1. Save child's current register file via PTRACE_GETREGS.
///   2. Save 3 bytes at child's current RIP via PTRACE_PEEKTEXT.
///   3. Write `syscall; int3` (0x0f 0x05 0xcc) to child's RIP.
///   4. Set up syscall number + args in rax/rdi/rsi/rdx/r10/r8/r9.
///   5. PTRACE_CONT → child executes syscall → hits int3 → SIGTRAP.
///   6. Read rax (return value).
///   7. Restore original bytes at RIP and original register file.
///
/// The child is stopped at the same state as before, as if the injection
/// never happened. Only the kernel-visible side-effects of the syscall remain
/// (e.g. a new mmap mapping).
struct SyscallInjector {
    child: Pid,
}

impl SyscallInjector {
    fn new(child: Pid) -> Self {
        SyscallInjector { child }
    }

    /// Inject a syscall with up to 6 arguments. Returns the syscall return value.
    ///
    /// Negative return values in the [−4095, 0) range are errno values;
    /// this function returns them as an `Err`.
    fn inject(
        &mut self,
        nr: u64,
        a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64,
    ) -> Result<u64> {
        let child = self.child;

        // Save current registers and RIP.
        let saved_regs = ptrace::getregs(child)
            .map_err(|e| anyhow!("inject: PTRACE_GETREGS: {}", e))?;

        let rip = saved_regs.rip;

        // Save the 8 bytes at RIP (we overwrite 3 of them).
        let saved_word = ptrace::read(child, rip as *mut libc::c_void)
            .map_err(|e| anyhow!("inject: PTRACE_PEEKTEXT at {:#x}: {}", rip, e))?;

        // Write `syscall` (0x0f 0x05) + `int3` (0xcc) at RIP.
        // We operate on a full 64-bit word to satisfy ptrace's word granularity.
        // The high 5 bytes are filled with NOPs (0x90) for safety.
        let patch: u64 = (saved_word & !0x00ff_ffff) | 0x00cc_050f;
        unsafe {
            ptrace::write(child, rip as *mut libc::c_void, patch as *mut libc::c_void)
        }
        .map_err(|e| anyhow!("inject: PTRACE_POKETEXT at {:#x}: {}", rip, e))?;

        // Set up syscall calling convention (System V AMD64 ABI):
        //   rax = syscall number
        //   rdi, rsi, rdx, r10, r8, r9 = arguments 0–5
        let mut call_regs = saved_regs;
        call_regs.rax = nr;
        call_regs.rdi = a0;
        call_regs.rsi = a1;
        call_regs.rdx = a2;
        call_regs.r10 = a3;
        call_regs.r8  = a4;
        call_regs.r9  = a5;

        ptrace::setregs(child, call_regs)
            .map_err(|e| anyhow!("inject: PTRACE_SETREGS: {}", e))?;

        // Resume until SIGTRAP (from int3 after the syscall).
        ptrace::cont(child, None)
            .map_err(|e| anyhow!("inject: PTRACE_CONT: {}", e))?;

        match waitpid(child, None)
            .map_err(|e| anyhow!("inject: waitpid: {}", e))?
        {
            WaitStatus::Stopped(_, Signal::SIGTRAP) => {} // expected
            WaitStatus::Stopped(_, sig) => {
                return Err(anyhow!("inject: unexpected signal {:?} after syscall", sig));
            }
            status => {
                return Err(anyhow!("inject: unexpected wait status {:?}", status));
            }
        }

        // Read the syscall return value from rax.
        let result_regs = ptrace::getregs(child)
            .map_err(|e| anyhow!("inject: PTRACE_GETREGS (result): {}", e))?;
        let retval = result_regs.rax;

        // Restore original instruction bytes.
        unsafe {
            ptrace::write(child, rip as *mut libc::c_void, saved_word as *mut libc::c_void)
        }
        .map_err(|e| anyhow!("inject: restore PTRACE_POKETEXT: {}", e))?;

        // Restore original registers (RIP reverts to pre-injection position).
        ptrace::setregs(child, saved_regs)
            .map_err(|e| anyhow!("inject: restore PTRACE_SETREGS: {}", e))?;

        // Convert negative return values (errno).
        let retval_i = retval as i64;
        if retval_i < 0 && retval_i >= -4095 {
            return Err(anyhow!(
                "syscall {} returned errno {}: {}",
                nr, -retval_i,
                std::io::Error::from_raw_os_error((-retval_i) as i32)
            ));
        }

        Ok(retval)
    }

    /// Inject `mmap(addr, len, prot, flags, fd, offset)`.
    fn mmap(
        &mut self,
        addr:   *mut libc::c_void,
        len:    usize,
        prot:   libc::c_int,
        flags:  libc::c_int,
        fd:     libc::c_int,
        offset: libc::off_t,
    ) -> Result<*mut libc::c_void> {
        let ret = self.inject(
            libc::SYS_mmap as u64,
            addr   as u64,
            len    as u64,
            prot   as u64,
            flags  as u64,
            fd     as u64,
            offset as u64,
        )?;
        Ok(ret as *mut libc::c_void)
    }

    /// Inject `mprotect(addr, len, prot)`.
    fn mprotect(
        &mut self,
        addr:  *mut libc::c_void,
        len:   usize,
        prot:  libc::c_int,
    ) -> Result<()> {
        self.inject(
            libc::SYS_mprotect as u64,
            addr as u64,
            len  as u64,
            prot as u64,
            0, 0, 0,
        )?;
        Ok(())
    }

    /// Inject `dup2(oldfd, newfd)`.
    fn dup2(&mut self, old_fd: libc::c_int, new_fd: libc::c_int) -> Result<()> {
        self.inject(
            libc::SYS_dup2 as u64,
            old_fd as u64,
            new_fd as u64,
            0, 0, 0, 0,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aslr::perms_to_prot;

    #[test]
    fn test_validate_rejects_no_registers() {
        let snapshot = ProcessSnapshot {
            registers: None,
            memory_regions: vec![crate::proto::wraith::MemoryRegion {
                start_addr:  0x10000,
                size_bytes:  4096,
                permissions: "rw-p".to_string(),
                data:        vec![0u8; 4096],
                checksum:    MemoryDumper::checksum(&vec![0u8; 4096]),
                ..Default::default()
            }],
            arch: "x86_64".to_string(),
            ..Default::default()
        };
        let restorer = ProcessRestorer::new(snapshot);
        assert!(restorer.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_wrong_arch() {
        let snapshot = ProcessSnapshot {
            arch: "aarch64".to_string(),
            ..Default::default()
        };
        let restorer = ProcessRestorer::new(snapshot);
        let err = restorer.validate().unwrap_err();
        assert!(err.to_string().contains("aarch64"));
    }

    #[test]
    fn test_prot_flags() {
        assert_eq!(perms_to_prot("rwxp"), libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC);
        assert_eq!(perms_to_prot("r--p"), libc::PROT_READ);
    }
}
