/// Phase 1 integration tests.
///
/// Tests that require ptrace (test_capture_*) are gated behind a runtime
/// check for ptrace availability. They print a skip message on CI rather
/// than failing, because many CI environments run with ptrace_scope=1.
///
/// Run with:
///   cargo test                      # all tests
///   cargo test -- --nocapture       # see println! output
///   sudo cargo test                 # run ptrace tests as root
use wraith_capturer::capturer::{Capturer, ProcessSnapshot};
use wraith_capturer::registers::Registers;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_registers(rip: u64, rsp: u64) -> Registers {
    Registers {
        rax: 0xdead, rbx: 0xbeef, rcx: 0xcafe, rdx: 0xbabe,
        rdi: 1, rsi: 2, rbp: rsp + 8, rsp,
        r8: 0, r9: 0, r10: 0, r11: 0, r12: 0, r13: 0, r14: 0, r15: 0,
        rip,
        rflags: 0x202,
        cs: 0x33, ss: 0x2b,
        ds: 0, es: 0, fs: 0, gs: 0,
        fs_base: 0x7000_0000_0000,
        gs_base: 0,
        fpu_state: vec![0u8; std::mem::size_of::<libc::user_fpregs_struct>()],
    }
}

fn make_snapshot(pid: i32) -> ProcessSnapshot {
    ProcessSnapshot {
        pid,
        process_name: "test_proc".to_string(),
        arch: "x86_64".to_string(),
        kernel_version: "Linux 6.1.0".to_string(),
        captured_at_ns: 1_700_000_000_000_000_000,
        registers: make_registers(0x40_1234, 0x7fff_0000_0ff8),
    }
}

// ---------------------------------------------------------------------------
// Unit tests (no ptrace, no root needed)
// ---------------------------------------------------------------------------

#[test]
fn test_registers_serialize_roundtrip() {
    let original = make_registers(0x40_1234, 0x7fff_0000_0ff8);
    let encoded = bincode::serialize(&original).expect("serialize");
    let decoded: Registers = bincode::deserialize(&encoded).expect("deserialize");

    assert_eq!(original.rax, decoded.rax);
    assert_eq!(original.rip, decoded.rip);
    assert_eq!(original.rsp, decoded.rsp);
    assert_eq!(original.fs_base, decoded.fs_base);
    assert_eq!(original.fpu_state.len(), decoded.fpu_state.len());
}

#[test]
fn test_registers_validate_valid() {
    let regs = make_registers(0x40_0000, 0x7fff_0000_0000);
    assert!(regs.validate().is_ok());
}

#[test]
fn test_registers_validate_kernel_rip() {
    // RIP in kernel space → validation must fail.
    let regs = make_registers(0xffff_8000_0000_0000, 0x7fff_0000_0000);
    assert!(regs.validate().is_err());
}

#[test]
fn test_registers_validate_zero_rsp() {
    let regs = make_registers(0x40_0000, 0);
    assert!(regs.validate().is_err());
}

#[test]
fn test_registers_validate_wrong_fpu_size() {
    let mut regs = make_registers(0x40_0000, 0x7fff_0000_0000);
    regs.fpu_state = vec![0u8; 100]; // should be 512
    assert!(regs.validate().is_err());
}

#[test]
fn test_snapshot_save_load_roundtrip() {
    let original = make_snapshot(99999);
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");

    Capturer::save(&original, tmp.path()).expect("save");
    let loaded = Capturer::load(tmp.path()).expect("load");

    assert_eq!(loaded.pid, original.pid);
    assert_eq!(loaded.process_name, original.process_name);
    assert_eq!(loaded.arch, original.arch);
    assert_eq!(loaded.captured_at_ns, original.captured_at_ns);
    assert_eq!(loaded.registers.rip, original.registers.rip);
    assert_eq!(loaded.registers.rsp, original.registers.rsp);
    assert_eq!(loaded.registers.fpu_state.len(), original.registers.fpu_state.len());
}

#[test]
fn test_snapshot_save_fails_bad_path() {
    let snap = make_snapshot(1);
    let result = Capturer::save(&snap, "/nonexistent/path/snapshot.bin");
    assert!(result.is_err());
}

#[test]
fn test_load_fails_on_missing_file() {
    let result = Capturer::load("/tmp/wraith_does_not_exist_xyz.bin");
    assert!(result.is_err());
}

#[test]
fn test_load_fails_on_garbage_data() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    std::fs::write(tmp.path(), b"this is not a valid bincode snapshot").expect("write");
    let result = Capturer::load(tmp.path());
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Integration tests (require ptrace — skipped if permissions are missing)
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn test_capture_sleep_process() {
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    let mut child = Command::new("sleep")
        .arg("3600")
        .spawn()
        .expect("failed to spawn sleep");

    let pid = child.id() as i32;
    thread::sleep(Duration::from_millis(150));

    let capturer = Capturer::new(pid);
    match capturer.capture() {
        Ok(snapshot) => {
            assert_eq!(snapshot.pid, pid);
            assert_eq!(snapshot.arch, "x86_64");
            assert!(snapshot.registers.rip > 0, "RIP must be non-zero");
            assert!(snapshot.registers.rsp > 0, "RSP must be non-zero");
            assert_eq!(
                snapshot.registers.fpu_state.len(),
                std::mem::size_of::<libc::user_fpregs_struct>(),
                "FPU state must be exactly sizeof(user_fpregs_struct) bytes"
            );
            // Process must still be alive after capture.
            assert!(wraith_capturer::utils::pid_exists(pid), "Process must survive capture");
        }
        Err(e) if e.to_string().contains("Permission denied") => {
            println!("SKIP test_capture_sleep_process: ptrace requires root or ptrace_scope=0");
        }
        Err(e) => panic!("Unexpected capture error: {:#}", e),
    }

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[cfg(target_os = "linux")]
fn test_capture_is_deterministic() {
    // Two captures of a sleeping process should yield the same registers
    // (modulo the stack pointer and instruction pointer, which may differ
    // if the process was in a different syscall each time, but rflags and
    // segment registers should be stable).
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    let mut child = Command::new("sleep")
        .arg("3600")
        .spawn()
        .expect("failed to spawn sleep");

    let pid = child.id() as i32;
    thread::sleep(Duration::from_millis(150));

    let capturer = Capturer::new(pid);

    match (capturer.capture(), capturer.capture()) {
        (Ok(s1), Ok(s2)) => {
            // Segment registers do not change between captures of a sleeping process.
            assert_eq!(s1.registers.cs, s2.registers.cs);
            assert_eq!(s1.registers.ss, s2.registers.ss);
            assert_eq!(s1.registers.fs_base, s2.registers.fs_base);
        }
        (Err(e), _) | (_, Err(e))
            if e.to_string().contains("Permission denied") =>
        {
            println!("SKIP test_capture_is_deterministic: ptrace requires root or ptrace_scope=0");
        }
        (Err(e), _) | (_, Err(e)) => panic!("Unexpected capture error: {:#}", e),
    }

    child.kill().ok();
    child.wait().ok();
}
