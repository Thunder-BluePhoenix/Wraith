use crate::error::{anyhow, Context, Result};
use crc::{Crc, CRC_64_ECMA_182};
use std::io::{Read, Seek, SeekFrom};

// CRC-64/ECMA-182 — same algorithm used in the Go transport layer for
// end-to-end integrity checks.
static CRC64: Crc<u64> = Crc::<u64>::new(&CRC_64_ECMA_182);

/// A single contiguous virtual memory region from /proc/<pid>/maps.
///
/// After calling `MemoryDumper::dump_region`, the `data` field contains
/// the raw page bytes and `checksum` contains a CRC-64 of those bytes.
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// Start virtual address (inclusive).
    pub start: u64,

    /// End virtual address (exclusive).
    pub end: u64,

    /// Permission string as reported by /proc/maps: e.g. "rwxp", "r--p".
    pub perms: String,

    /// Absolute path of the backing file, if any.
    pub backing_file: Option<String>,

    /// Offset within the backing file where this mapping starts.
    pub offset: u64,

    /// Semantic classification: "heap", "stack", "vdso", "anon", "file", etc.
    pub region_type: String,

    /// Raw page bytes — populated by `MemoryDumper::dump_region`.
    pub data: Vec<u8>,

    /// CRC-64/ECMA-182 of `data` — populated after `dump_region`.
    pub checksum: u64,
}

impl MemoryRegion {
    /// Size of the region in bytes.
    pub fn size(&self) -> u64 {
        self.end - self.start
    }

    /// True if this region has read permission.
    pub fn is_readable(&self) -> bool {
        self.perms.starts_with('r')
    }
}

/// Handles parsing and reading of process memory.
pub struct MemoryDumper;

impl MemoryDumper {
    /// Parse `/proc/<pid>/maps` and return all regions (without page data).
    ///
    /// Page data is read separately via `dump_region` while the process is frozen.
    pub fn parse_maps(pid: i32) -> Result<Vec<MemoryRegion>> {
        let path = format!("/proc/{}/maps", pid);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Cannot read {}", path))?;

        let mut regions = Vec::new();

        for (line_no, line) in content.lines().enumerate() {
            match Self::parse_maps_line(line) {
                Ok(Some(region)) => regions.push(region),
                Ok(None) => {} // blank line
                Err(e) => log::warn!("Skipping malformed maps line {}: {}", line_no + 1, e),
            }
        }

        log::debug!("Parsed {} memory regions for PID {}", regions.len(), pid);
        Ok(regions)
    }

    /// Read the actual page bytes for a region from `/proc/<pid>/mem`.
    ///
    /// The target process **must** be ptrace-stopped before calling this.
    /// Regions that cannot be read (e.g. due to kernel restrictions) return
    /// an error; callers should decide whether to skip or abort.
    pub fn dump_region(pid: i32, region: &MemoryRegion) -> Result<Vec<u8>> {
        let path = format!("/proc/{}/mem", pid);
        let mut f = std::fs::File::open(&path)
            .with_context(|| format!("Cannot open {}", path))?;

        f.seek(SeekFrom::Start(region.start))
            .with_context(|| format!("Cannot seek to {:#x} in {}", region.start, path))?;

        let size = region.size() as usize;
        let mut buf = vec![0u8; size];

        f.read_exact(&mut buf).with_context(|| {
            format!(
                "Cannot read {} bytes at {:#x}–{:#x} from {}",
                size, region.start, region.end, path
            )
        })?;

        Ok(buf)
    }

    /// Compute CRC-64/ECMA-182 checksum of a byte slice.
    pub fn checksum(data: &[u8]) -> u64 {
        CRC64.checksum(data)
    }

    /// Parse a single line of /proc/pid/maps.
    ///
    /// Format:
    ///   addr_start-addr_end  perms  offset  dev  inode  [pathname]
    ///   7f0000001000-7f0000002000  r--p  00001000  08:01  1234  /lib/libc.so.6
    fn parse_maps_line(line: &str) -> Result<Option<MemoryRegion>> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(None);
        }

        // Split into at most 6 fields (pathname may contain spaces).
        let mut parts = line.splitn(6, ' ').filter(|s| !s.is_empty());

        let addr_range = parts
            .next()
            .ok_or_else(|| anyhow!("missing address range in maps line: {:?}", line))?;
        let perms = parts
            .next()
            .ok_or_else(|| anyhow!("missing permissions in maps line: {:?}", line))?;
        let offset_str = parts
            .next()
            .ok_or_else(|| anyhow!("missing offset in maps line: {:?}", line))?;
        let _dev = parts.next(); // major:minor — not needed for v1
        let _inode = parts.next(); // inode number — not needed for v1
        let pathname = parts.next().unwrap_or("").trim().to_string();

        // Parse address range.
        let (start_str, end_str) = addr_range
            .split_once('-')
            .ok_or_else(|| anyhow!("malformed address range: {:?}", addr_range))?;

        let start = u64::from_str_radix(start_str, 16)
            .with_context(|| format!("bad start address: {:?}", start_str))?;
        let end = u64::from_str_radix(end_str, 16)
            .with_context(|| format!("bad end address: {:?}", end_str))?;

        let offset = u64::from_str_radix(offset_str, 16)
            .with_context(|| format!("bad offset: {:?}", offset_str))?;

        let backing_file = if pathname.starts_with('/') || pathname.starts_with('.') {
            Some(pathname.clone())
        } else {
            None
        };

        let region_type = classify_region(&pathname).to_string();

        Ok(Some(MemoryRegion {
            start,
            end,
            perms: perms.to_string(),
            backing_file,
            offset,
            region_type,
            data: Vec::new(),
            checksum: 0,
        }))
    }

    /// True if this region should be skipped during capture.
    ///
    /// Skipped regions:
    ///   - `[vsyscall]`  — kernel virtual syscall page; EPERM on read
    ///   - `[vvar]`      — kernel variables page; EPERM on read
    ///   - No read perm  — cannot read, cannot restore
    ///   - Zero size     — degenerate entry
    pub fn should_skip(region: &MemoryRegion) -> bool {
        if region.size() == 0 {
            return true;
        }
        if !region.is_readable() {
            return true;
        }
        matches!(
            region.region_type.as_str(),
            "vsyscall" | "vvar"
        )
    }
}

/// Classify a memory region based on its /proc/maps pathname field.
fn classify_region(pathname: &str) -> &'static str {
    match pathname {
        "[heap]"     => "heap",
        "[stack]"    => "stack",
        "[vdso]"     => "vdso",
        "[vsyscall]" => "vsyscall",
        "[vvar]"     => "vvar",
        p if p.starts_with('/') => "file",
        p if p.is_empty()       => "anon",
        _                       => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_maps_line_file_backed() {
        let line = "7f1234560000-7f1234561000 r--p 00001000 08:01 9876 /lib/libc.so.6";
        let region = MemoryDumper::parse_maps_line(line).unwrap().unwrap();
        assert_eq!(region.start, 0x7f1234560000);
        assert_eq!(region.end,   0x7f1234561000);
        assert_eq!(region.perms, "r--p");
        assert_eq!(region.offset, 0x1000);
        assert_eq!(region.backing_file.as_deref(), Some("/lib/libc.so.6"));
        assert_eq!(region.region_type, "file");
        assert!(region.is_readable());
    }

    #[test]
    fn test_parse_maps_line_heap() {
        let line = "55f4a1b00000-55f4a1c00000 rw-p 00000000 00:00 0  [heap]";
        let region = MemoryDumper::parse_maps_line(line).unwrap().unwrap();
        assert_eq!(region.region_type, "heap");
        assert!(region.backing_file.is_none());
        assert!(region.is_readable());
    }

    #[test]
    fn test_parse_maps_line_anon() {
        let line = "7ffe00001000-7ffe00002000 rw-p 00000000 00:00 0";
        let region = MemoryDumper::parse_maps_line(line).unwrap().unwrap();
        assert_eq!(region.region_type, "anon");
    }

    #[test]
    fn test_parse_maps_line_no_perms_skipped() {
        let line = "7f0000001000-7f0000002000 ---p 00000000 00:00 0";
        let region = MemoryDumper::parse_maps_line(line).unwrap().unwrap();
        assert!(MemoryDumper::should_skip(&region));
    }

    #[test]
    fn test_should_skip_vsyscall() {
        let line = "ffffffffff600000-ffffffffff601000 --xp 00000000 00:00 0  [vsyscall]";
        let region = MemoryDumper::parse_maps_line(line).unwrap().unwrap();
        assert!(MemoryDumper::should_skip(&region));
    }

    #[test]
    fn test_checksum_deterministic() {
        let data = b"hello wraith";
        let c1 = MemoryDumper::checksum(data);
        let c2 = MemoryDumper::checksum(data);
        assert_eq!(c1, c2);
        assert_ne!(c1, 0);
    }

    #[test]
    fn test_checksum_changes_on_data_change() {
        let a = MemoryDumper::checksum(b"page content A");
        let b = MemoryDumper::checksum(b"page content B");
        assert_ne!(a, b);
    }
}
