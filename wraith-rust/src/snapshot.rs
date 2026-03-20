/// SnapshotBuilder converts internal Wraith types into the Protobuf
/// `ProcessSnapshot` message defined in proto/wraith.proto.
///
/// This is the boundary between the kernel-facing capture code (registers.rs,
/// memory.rs, fd_enum.rs) and the serialization layer (prost + protobuf).
///
/// Usage:
///   ```rust
///   let snapshot = SnapshotBuilder::new(pid)
///       .registers(regs)
///       .memory_regions(regions)
///       .file_descriptors(fds)
///       .metadata(name, kernel)
///       .build()?;
///   ```
use crate::error::{anyhow, Result};
use crate::fd_enum::{FileDescriptor, FdType};
use crate::memory::MemoryRegion;
use crate::proto::wraith;
use crate::registers::Registers;
use crate::utils;

pub struct SnapshotBuilder {
    pid:          i32,
    registers:    Option<Registers>,
    regions:      Vec<MemoryRegion>,
    fds:          Vec<FileDescriptor>,
    process_name: String,
    kernel:       String,
}

impl SnapshotBuilder {
    pub fn new(pid: i32) -> Self {
        SnapshotBuilder {
            pid,
            registers:    None,
            regions:      Vec::new(),
            fds:          Vec::new(),
            process_name: utils::process_name(pid),
            kernel:       kernel_version(),
        }
    }

    pub fn registers(mut self, regs: Registers) -> Self {
        self.registers = Some(regs);
        self
    }

    pub fn memory_regions(mut self, regions: Vec<MemoryRegion>) -> Self {
        self.regions = regions;
        self
    }

    pub fn file_descriptors(mut self, fds: Vec<FileDescriptor>) -> Self {
        self.fds = fds;
        self
    }

    /// Assemble the final protobuf `ProcessSnapshot`.
    ///
    /// Fails if registers were not provided (they are always required).
    pub fn build(self) -> Result<wraith::ProcessSnapshot> {
        let registers = self
            .registers
            .ok_or_else(|| anyhow!("SnapshotBuilder: registers are required"))?;

        let metadata = wraith::SnapshotMetadata {
            machine_hostname:      hostname(),
            process_name:          self.process_name.clone(),
            process_age_seconds:   process_age_seconds(self.pid),
            thread_count:          1, // v1: single-threaded only
            virtual_memory_bytes:  vm_size_bytes(self.pid),
            resident_memory_bytes: vm_rss_bytes(self.pid),
        };

        let proto_regs = regs_to_proto(&registers);

        let proto_regions: Vec<wraith::MemoryRegion> =
            self.regions.iter().map(region_to_proto).collect();

        let proto_fds: Vec<wraith::FileDescriptor> =
            self.fds.iter().map(fd_to_proto).collect();

        let snapshot = wraith::ProcessSnapshot {
            pid:              self.pid as u32,
            uid:              get_uid(self.pid),
            arch:             std::env::consts::ARCH.to_string(),
            kernel_version:   self.kernel,
            captured_at_ns:   utils::timestamp_ns(),
            metadata:         Some(metadata),
            registers:        Some(proto_regs),
            memory_regions:   proto_regions,
            file_descriptors: proto_fds,
            // Checksum of the full encoded snapshot is set by the
            // transport layer (Phase 3) after serialization, because
            // computing it here would require a second encode pass.
            checksum:         0,
            snapshot_version: "1.0".to_string(),
        };

        Ok(snapshot)
    }
}

// ── Internal conversion functions ────────────────────────────────────────────

fn regs_to_proto(r: &Registers) -> wraith::Registers {
    wraith::Registers {
        rax: r.rax, rbx: r.rbx, rcx: r.rcx, rdx: r.rdx,
        rdi: r.rdi, rsi: r.rsi, rbp: r.rbp, rsp: r.rsp,
        r8:  r.r8,  r9:  r.r9,  r10: r.r10, r11: r.r11,
        r12: r.r12, r13: r.r13, r14: r.r14, r15: r.r15,
        rip:    r.rip,
        rflags: r.rflags,
        cs: r.cs, ss: r.ss, ds: r.ds, es: r.es, fs: r.fs, gs: r.gs,
        fs_base: r.fs_base,
        gs_base: r.gs_base,
        fpu_state: r.fpu_state.clone(),
    }
}

fn region_to_proto(r: &MemoryRegion) -> wraith::MemoryRegion {
    wraith::MemoryRegion {
        start_addr:   r.start,
        end_addr:     r.end,
        size_bytes:   r.size(),
        permissions:  r.perms.clone(),
        region_type:  r.region_type.clone(),
        backing_file: r.backing_file.clone().unwrap_or_default(),
        file_offset:  r.offset,
        data:         r.data.clone(),
        checksum:     r.checksum,
    }
}

fn fd_to_proto(f: &FileDescriptor) -> wraith::FileDescriptor {
    wraith::FileDescriptor {
        fd_num:      f.fd,
        fd_type:     f.fd_type.to_string(),
        path:        f.path.clone(),
        file_offset: f.offset,
        flags:       f.flags as u32,
    }
}

// ── OS helpers ───────────────────────────────────────────────────────────────

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn kernel_version() -> String {
    std::fs::read_to_string("/proc/version")
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn get_uid(pid: i32) -> u32 {
    parse_status_field(pid, "Uid:")
        .and_then(|s| s.split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

fn process_age_seconds(pid: i32) -> u64 {
    // Approximate: read /proc/<pid>/stat field 22 (starttime in clock ticks)
    // and compare against /proc/uptime. Fail silently.
    (|| -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
        let fields: Vec<&str> = stat.split_whitespace().collect();
        let start_ticks: u64 = fields.get(21)?.parse().ok()?;

        let uptime_str = std::fs::read_to_string("/proc/uptime").ok()?;
        let uptime_secs: f64 = uptime_str.split_whitespace().next()?.parse().ok()?;

        let hz = 100u64; // SC_CLK_TCK, usually 100 on Linux
        let start_secs = start_ticks / hz;
        let uptime = uptime_secs as u64;

        Some(uptime.saturating_sub(start_secs))
    })()
    .unwrap_or(0)
}

fn vm_size_bytes(pid: i32) -> u64 {
    parse_status_kb(pid, "VmSize:").unwrap_or(0) * 1024
}

fn vm_rss_bytes(pid: i32) -> u64 {
    parse_status_kb(pid, "VmRSS:").unwrap_or(0) * 1024
}

fn parse_status_field(pid: i32, field: &str) -> Option<String> {
    let content = std::fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    content
        .lines()
        .find(|l| l.starts_with(field))
        .map(|l| l[field.len()..].trim().to_string())
}

fn parse_status_kb(pid: i32, field: &str) -> Option<u64> {
    parse_status_field(pid, field)
        .and_then(|s| s.split_whitespace().next().and_then(|n| n.parse().ok()))
}
