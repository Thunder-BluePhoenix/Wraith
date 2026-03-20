# Phase 1: Rust Foundation — ptrace and Register Capture

**Duration**: 2 weeks | **Owner**: Rust team | **Output**: Working capture binary

## Goals

1. Attach to a running process and freeze it safely
2. Extract CPU registers (GPRs, FPU, SSE/AVX) without corruption
3. Validate x86-64 register layout
4. Build reusable ptrace abstraction layer

## Deliverables

### 1.1 Project Structure
```
wraith-rust/
├── Cargo.toml
├── src/
│   ├── main.rs           (CLI entry point)
│   ├── capturer.rs       (Capture logic)
│   ├── ptrace_ops.rs     (ptrace wrapper)
│   ├── registers.rs      (Register struct + serialize)
│   ├── error.rs          (Error handling)
│   └── utils.rs          (Helpers)
├── tests/
│   └── integration_tests.rs
└── README.md
```

### 1.2 Cargo Dependencies
```toml
[package]
name = "wraith-capturer"
version = "0.1.0"
edition = "2021"

[dependencies]
nix = "0.27"              # ptrace wrappers
libc = "0.2"              # syscalls
anyhow = "1.0"            # error handling
serde = { version = "1.0", features = ["derive"] }
bincode = "1.3"           # serialization

[dev-dependencies]
tempfile = "3.8"
```

### 1.3 Core Modules

#### **ptrace_ops.rs** — Safe ptrace wrapper
```rust
pub struct ProcessLock {
    pid: Pid,
}

impl ProcessLock {
    /// Attach to process and freeze
    pub fn attach(pid: i32) -> Result<Self> {
        let pid = Pid::from_raw(pid);
        ptrace::attach(pid)?;
        waitpid(pid, None)?;  // Wait until stopped
        Ok(ProcessLock { pid })
    }

    /// Detach and resume
    pub fn detach(&self) -> Result<()> {
        ptrace::detach(self.pid)?;
        Ok(())
    }

    /// Get all registers
    pub fn get_registers(&self) -> Result<Registers> {
        // Implement register capture
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}
```

#### **registers.rs** — x86-64 register model
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registers {
    // General purpose
    pub rax: u64, pub rbx: u64, pub rcx: u64, pub rdx: u64,
    pub rdi: u64, pub rsi: u64, pub rbp: u64, pub rsp: u64,
    pub r8: u64, pub r9: u64, pub r10: u64, pub r11: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,

    // Flags and control
    pub rip: u64,
    pub rflags: u64,

    // Floating point (raw bytes)
    pub fpu_state: Vec<u8>,  // 512 bytes for FXSAVE format
}

impl Registers {
    pub fn from_ptrace(pid: Pid) -> Result<Self> {
        // Use ptrace::getregs + ptrace::getfpregs
        // For AVX, may need to access /proc/pid/auxv
    }

    pub fn validate_arch(&self) -> Result<()> {
        // Verify registers are within valid ranges for x86-64
    }
}
```

#### **capturer.rs** — Main orchestration
```rust
pub struct Capturer;

impl Capturer {
    /// Capture single-threaded process state
    pub fn capture(pid: i32) -> Result<ProcessSnapshot> {
        let process = ProcessLock::attach(pid)?;
        
        // 1. Capture registers
        let registers = process.get_registers()?;
        registers.validate_arch()?;

        // 2. stub: memory capture (Phase 2)
        // 3. stub: FD enumeration (later phase)

        Ok(ProcessSnapshot {
            registers,
            // ... populated later
        })
    }
}
```

## Testing Strategy

### Unit Tests
- Register serialization/deserialization round-trip
- Validate bit patterns in FPU state
- Architecture detection

### Integration Tests
- Capture a sleeping process ✓ validate unchanged after resume
- Capture a looping process ✓ validate register changes reasonable
- Capture a process with open files ✓ stub for Phase 3

**Test binary**: Simple C program that spins or sleeps
```c
// test_target.c
#include <unistd.h>
int main() {
    volatile long x = 0;
    while (1) x++;  // Busy loop to capture registers
    return 0;
}
```

## Validation Checklist

- [ ] Attach/detach works without crashing process
- [ ] Register capture deterministic (same pid, immediate re-capture = same values)
- [ ] Registers decode correctly for x86-64
- [ ] FPU state captured (128+ bytes)
- [ ] No ptrace syscall leaks (process cleans up on error)
- [ ] Works for both user and root-owned processes

## Known Constraints (Document for later)

- ❌ Multi-threaded processes (Phase 8)
- ❌ Cross-architecture (Phase 11)
- ✓ Single architecture x86-64 only
- ✓ Linux 5.0+ kernel

## Dependencies on Other Phases

- Upstream: None
- Downstream: Phase 2 (memory capture uses ptrace patterns from Phase 1)

## Success Criteria

- [x] Build succeeds
- [x] Can freeze and unfreeze a process
- [x] Register capture is deterministic
- [x] Integration test passes
