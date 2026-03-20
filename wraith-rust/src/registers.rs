use crate::error::{anyhow, Result};
use nix::sys::ptrace;
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};

/// Complete x86-64 CPU register state for one thread.
///
/// Captured via:
///   - ptrace(PTRACE_GETREGS)   → general-purpose registers
///   - ptrace(PTRACE_GETFPREGS) → FPU / SSE state (FXSAVE format, 512 bytes)
///
/// All fields are preserved exactly as-is; no interpretation is done here.
/// The restorer (Phase 4) feeds them back via ptrace(PTRACE_SETREGS / PTRACE_SETFPREGS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registers {
    // General-purpose registers (x86-64)
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8:  u64,
    pub r9:  u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,

    // Instruction pointer and flags
    pub rip:    u64,
    pub rflags: u64,  // stored as eflags in libc's user_regs_struct

    // Segment registers (needed for correct restore)
    pub cs:      u64,
    pub ss:      u64,
    pub ds:      u64,
    pub es:      u64,
    pub fs:      u64,
    pub gs:      u64,
    pub fs_base: u64,  // TLS base for the main thread
    pub gs_base: u64,

    // Raw FXSAVE block: FPU + MMX + SSE (xmm0–xmm15) state.
    // Always exactly 512 bytes on x86-64.
    pub fpu_state: Vec<u8>,
}

impl Registers {
    /// Capture register state from a process that is already stopped via ptrace.
    pub fn from_ptrace(pid: Pid) -> Result<Self> {
        let gp = ptrace::getregs(pid)
            .map_err(|e| anyhow!("PTRACE_GETREGS failed for PID {}: {}", pid, e))?;

        let fp = ptrace::getfpregs(pid)
            .map_err(|e| anyhow!("PTRACE_GETFPREGS failed for PID {}: {}", pid, e))?;

        // Safety: user_fpregs_struct is a plain C struct with no padding traps;
        // we copy the raw bytes for serialization. The restorer reconstructs it
        // via the same cast in the opposite direction.
        let fpu_state = unsafe {
            let ptr = &fp as *const libc::user_fpregs_struct as *const u8;
            let len = std::mem::size_of::<libc::user_fpregs_struct>();
            std::slice::from_raw_parts(ptr, len).to_vec()
        };

        Ok(Registers {
            rax: gp.rax,
            rbx: gp.rbx,
            rcx: gp.rcx,
            rdx: gp.rdx,
            rdi: gp.rdi,
            rsi: gp.rsi,
            rbp: gp.rbp,
            rsp: gp.rsp,
            r8:  gp.r8,
            r9:  gp.r9,
            r10: gp.r10,
            r11: gp.r11,
            r12: gp.r12,
            r13: gp.r13,
            r14: gp.r14,
            r15: gp.r15,
            rip:    gp.rip,
            rflags: gp.eflags,
            cs:      gp.cs,
            ss:      gp.ss,
            ds:      gp.ds,
            es:      gp.es,
            fs:      gp.fs,
            gs:      gp.gs,
            fs_base: gp.fs_base,
            gs_base: gp.gs_base,
            fpu_state,
        })
    }

    /// Sanity-check the captured register state.
    ///
    /// These checks catch obviously corrupted captures before they propagate
    /// to serialization or restore. They are not exhaustive.
    pub fn validate(&self) -> Result<()> {
        // RIP must be in 64-bit user-space range (below the 47-bit canonical boundary).
        // Kernel addresses start at 0xffff_8000_0000_0000.
        if self.rip >= (1u64 << 47) {
            return Err(anyhow!(
                "RIP {:#018x} is in kernel space — capture corrupted or process is in a syscall",
                self.rip
            ));
        }

        // RSP must be non-zero (every running process has a stack).
        if self.rsp == 0 {
            return Err(anyhow!("RSP is zero — no valid stack pointer"));
        }

        // FPU state must be exactly sizeof(user_fpregs_struct) bytes.
        let expected_fpu = std::mem::size_of::<libc::user_fpregs_struct>();
        if !self.fpu_state.is_empty() && self.fpu_state.len() != expected_fpu {
            return Err(anyhow!(
                "FPU state length {} does not match expected {} bytes",
                self.fpu_state.len(),
                expected_fpu
            ));
        }

        Ok(())
    }
}
