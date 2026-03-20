use crate::error::{anyhow, Result};
use crate::fd_enum;
use crate::memory::MemoryDumper;
use crate::proto::wraith;
use crate::ptrace_ops::ProcessLock;
use crate::snapshot::SnapshotBuilder;
use crate::utils;
use prost::Message;
use std::path::Path;

/// Orchestrates the full capture of a single-threaded process.
///
/// ## Phase 2 capture sequence
///
/// 1. Pre-flight: architecture check, PID existence.
/// 2. Attach via ptrace — process is now frozen.
/// 3. Read registers (PTRACE_GETREGS / PTRACE_GETFPREGS).
/// 4. Parse /proc/<pid>/maps and read all capturable memory regions.
/// 5. Enumerate open file descriptors via /proc/<pid>/fd.
/// 6. Detach — process resumes.
/// 7. Assemble and return a Protobuf `ProcessSnapshot`.
///
/// The process is frozen for steps 2–5 only. On any error, `ProcessLock::Drop`
/// automatically detaches and resumes the source — rollback is guaranteed.
pub struct Capturer {
    pid: i32,
}

impl Capturer {
    pub fn new(pid: i32) -> Self {
        Capturer { pid }
    }

    /// Capture a complete process snapshot and return it as a Protobuf message.
    pub fn capture(&self) -> Result<wraith::ProcessSnapshot> {
        utils::assert_x86_64()?;

        let name = utils::process_name(self.pid);
        log::info!("Starting capture of PID {} ({})", self.pid, name);

        // ── Step 1: Parse memory map before freezing ─────────────────────────
        // /proc/pid/maps is readable without ptrace; getting the map before
        // attaching avoids a race where a new mapping appears mid-read.
        let mut regions = MemoryDumper::parse_maps(self.pid)?;

        // ── Step 2–5: Everything inside the ptrace freeze window ─────────────
        let process = ProcessLock::attach(self.pid)?;

        let registers = process.capture_registers()?;

        // Read page data for each capturable region.
        let mut skipped = 0usize;
        for region in &mut regions {
            if MemoryDumper::should_skip(region) {
                skipped += 1;
                continue;
            }
            match MemoryDumper::dump_region(self.pid, region) {
                Ok(data) => {
                    region.checksum = MemoryDumper::checksum(&data);
                    region.data = data;
                }
                Err(e) => {
                    // Some regions (e.g. vdso on hardened kernels) can fail even
                    // with ptrace. Log and skip rather than aborting the capture.
                    log::warn!(
                        "Skipping region {:#x}–{:#x} ({}): {}",
                        region.start, region.end, region.region_type, e
                    );
                    skipped += 1;
                }
            }
        }

        // Remove regions with no data (skipped above).
        regions.retain(|r| !r.data.is_empty());

        // Enumerate FDs while still frozen for a consistent view.
        let fds = fd_enum::enumerate_fds(self.pid)?;

        // ── Step 6: Detach — process resumes ─────────────────────────────────
        // ProcessLock drops here; Drop calls ptrace::detach automatically.
        drop(process);

        // ── Step 7: Assemble snapshot ─────────────────────────────────────────
        let total_bytes: usize = regions.iter().map(|r| r.data.len()).sum();

        let snapshot = SnapshotBuilder::new(self.pid)
            .registers(registers)
            .memory_regions(regions)
            .file_descriptors(fds)
            .build()?;

        log::info!(
            "Capture complete: PID {} — {} memory regions ({} MB), {} fds, {} regions skipped",
            self.pid,
            snapshot.memory_regions.len(),
            total_bytes / 1024 / 1024,
            snapshot.file_descriptors.len(),
            skipped,
        );

        Ok(snapshot)
    }

    /// Serialize a snapshot to Protobuf bytes and write to disk.
    pub fn save(snapshot: &wraith::ProcessSnapshot, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let bytes = snapshot.encode_to_vec();
        std::fs::write(path, &bytes)
            .map_err(|e| anyhow!("Failed to write snapshot to {}: {}", path.display(), e))?;
        log::info!(
            "Snapshot saved → {} ({} bytes, {} memory regions)",
            path.display(),
            bytes.len(),
            snapshot.memory_regions.len(),
        );
        Ok(())
    }

    /// Read and deserialize a Protobuf snapshot from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<wraith::ProcessSnapshot> {
        let path = path.as_ref();
        let data = std::fs::read(path)
            .map_err(|e| anyhow!("Failed to read snapshot from {}: {}", path.display(), e))?;
        wraith::ProcessSnapshot::decode(data.as_slice())
            .map_err(|e| anyhow!("Failed to decode snapshot from {}: {}", path.display(), e))
    }

    /// Compute a summary string for quick inspection without full decode.
    pub fn summary(snapshot: &wraith::ProcessSnapshot) -> String {
        let regs = snapshot.registers.as_ref();
        format!(
            "pid={} arch={} regions={} fds={} rip={:#x} rsp={:#x}",
            snapshot.pid,
            snapshot.arch,
            snapshot.memory_regions.len(),
            snapshot.file_descriptors.len(),
            regs.map(|r| r.rip).unwrap_or(0),
            regs.map(|r| r.rsp).unwrap_or(0),
        )
    }
}
