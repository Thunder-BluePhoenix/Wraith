use crate::error::{anyhow, Result};
use crate::ptrace_ops::ProcessLock;
use crate::registers::Registers;
use crate::utils;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Snapshot of a process at a single point in time.
///
/// ## Phase evolution
///
/// - Phase 1 (current): registers only.
/// - Phase 2: adds `memory_regions` and `file_descriptors`.
/// - Phase 2: switches serialization from bincode to Protobuf (wraith.proto).
///
/// The bincode format used here is temporary and intentionally not
/// guaranteed stable across builds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessSnapshot {
    /// Original PID on the source machine.
    pub pid: i32,

    /// Human-readable process name (/proc/<pid>/comm).
    pub process_name: String,

    /// CPU architecture string ("x86_64").
    pub arch: String,

    /// Kernel version string from /proc/version.
    pub kernel_version: String,

    /// Unix timestamp in nanoseconds when the snapshot was taken.
    pub captured_at_ns: u64,

    /// CPU register state (general-purpose + FPU).
    pub registers: Registers,
    // Phase 2 additions (not yet implemented):
    // pub memory_regions: Vec<crate::memory::MemoryRegion>,
    // pub file_descriptors: Vec<crate::fd_enum::FileDescriptor>,
}

/// Orchestrates the capture of a single-threaded process.
///
/// Attach → freeze → read registers → detach (resume).
/// The process is frozen for the minimum time needed to read its state.
pub struct Capturer {
    pid: i32,
}

impl Capturer {
    pub fn new(pid: i32) -> Self {
        Capturer { pid }
    }

    /// Capture a complete process snapshot.
    ///
    /// The process is frozen for the duration of the call.
    /// On any error the `ProcessLock` is dropped, resuming the process.
    pub fn capture(&self) -> Result<ProcessSnapshot> {
        utils::assert_x86_64()?;

        let name = utils::process_name(self.pid);
        let arch = utils::process_arch(self.pid).unwrap_or_else(|_| "x86_64".to_string());
        let kernel = kernel_version();

        log::info!("Capturing PID {} ({})", self.pid, name);

        // Attach freezes the process. ProcessLock::Drop resumes it on any exit path.
        let process = ProcessLock::attach(self.pid)?;

        let registers = process.capture_registers()?;

        // Phase 2: capture memory regions while process is still frozen.
        // Phase 2: enumerate file descriptors while process is still frozen.

        // ProcessLock drops here → process resumes.
        drop(process);

        let snapshot = ProcessSnapshot {
            pid: self.pid,
            process_name: name,
            arch,
            kernel_version: kernel,
            captured_at_ns: utils::timestamp_ns(),
            registers,
        };

        log::info!(
            "Captured PID {} — {} bytes (Phase 1: registers only)",
            self.pid,
            bincode::serialized_size(&snapshot).unwrap_or(0),
        );

        Ok(snapshot)
    }

    /// Write a snapshot to disk using bincode encoding.
    ///
    /// Phase 2 will replace this with Protobuf serialization.
    pub fn save(snapshot: &ProcessSnapshot, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let encoded = bincode::serialize(snapshot)
            .map_err(|e| anyhow!("Failed to serialize snapshot: {}", e))?;
        std::fs::write(path, &encoded)
            .map_err(|e| anyhow!("Failed to write snapshot to {}: {}", path.display(), e))?;
        log::info!(
            "Snapshot saved → {} ({} bytes)",
            path.display(),
            encoded.len()
        );
        Ok(())
    }

    /// Load a snapshot from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<ProcessSnapshot> {
        let path = path.as_ref();
        let data = std::fs::read(path)
            .map_err(|e| anyhow!("Failed to read snapshot from {}: {}", path.display(), e))?;
        let snapshot: ProcessSnapshot = bincode::deserialize(&data)
            .map_err(|e| anyhow!("Failed to deserialize snapshot from {}: {}", path.display(), e))?;
        Ok(snapshot)
    }
}

/// Read the kernel version string from /proc/version.
fn kernel_version() -> String {
    std::fs::read_to_string("/proc/version")
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| "unknown".to_string())
}
