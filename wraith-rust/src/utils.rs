use crate::error::{bail, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Assert the current architecture is x86-64 at runtime.
/// (compile_error! in lib.rs catches it at build time; this is a belt-and-suspenders
/// runtime guard for environments where cross-compilation sneaks through.)
pub fn assert_x86_64() -> Result<()> {
    if std::env::consts::ARCH != "x86_64" {
        bail!(
            "wraith-capturer requires x86-64; current arch = {}",
            std::env::consts::ARCH
        );
    }
    Ok(())
}

/// Return true if the given PID exists in /proc.
pub fn pid_exists(pid: i32) -> bool {
    Path::new(&format!("/proc/{}", pid)).exists()
}

/// Current time in nanoseconds since Unix epoch.
pub fn timestamp_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Read the process name from /proc/<pid>/comm.
/// Returns "<unknown>" on any error (non-fatal).
pub fn process_name(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

/// Read process architecture from /proc/<pid>/exe ELF header.
/// Returns the architecture string ("x86_64") or an error.
pub fn process_arch(pid: i32) -> Result<String> {
    use std::fs::File;
    use std::io::Read;

    let exe_path = format!("/proc/{}/exe", pid);
    let mut f = File::open(&exe_path)
        .with_context(|| format!("Cannot open {}", exe_path))?;

    let mut magic = [0u8; 20];
    f.read_exact(&mut magic)?;

    // ELF magic: 0x7f 'E' 'L' 'F'
    if &magic[0..4] != b"\x7fELF" {
        bail!("PID {} exe does not appear to be an ELF binary", pid);
    }

    // Byte 4: EI_CLASS  (1 = 32-bit, 2 = 64-bit)
    // Byte 18-19: e_machine (0x3e = x86-64, 0xb7 = AArch64)
    let machine = u16::from_le_bytes([magic[18], magic[19]]);
    let arch = match machine {
        0x003e => "x86_64",
        0x00b7 => "aarch64",
        0x0028 => "arm",
        _ => "unknown",
    };

    Ok(arch.to_string())
}

// Bring Context into scope for the with_context call above.
use crate::error::Context;
