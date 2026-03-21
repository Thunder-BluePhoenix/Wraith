// Package aslr handles address-space layout validation for process restore.
//
// The key challenge in process restoration is that ASLR (Address Space Layout
// Randomisation) means the destination kernel will place mappings at different
// virtual addresses than the source. Wraith v1 sidesteps this by:
//
//   1. Using MAP_FIXED when re-creating every mapping, forcing exact addresses.
//   2. Running the restore from outside the target address space (parent ptrace).
//   3. Validating the snapshot layout BEFORE forking so we can reject hopeless
//      cases early (addresses above 47-bit boundary, overlapping regions, etc).
//
// v2 limitations (see phase8.md Phase 8.3) — handling when MAP_FIXED fails
// because the kernel refuses to place memory at a requested address — are
// tracked but intentionally out of scope here.

use crate::error::{anyhow, Result};
use crate::proto::wraith::ProcessSnapshot;

/// Byte-addressed range: [start, end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddrRange {
    pub start: u64,
    pub end:   u64,
}

impl AddrRange {
    pub fn size(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn overlaps(&self, other: &AddrRange) -> bool {
        self.start < other.end && other.start < self.end
    }
}

/// Sorted, validated view of the address ranges that need to be recreated.
///
/// Built from a `ProcessSnapshot`; use `validate()` before attempting a restore.
pub struct AddressSpaceLayout {
    /// Sorted by start address.
    pub ranges: Vec<AddrRange>,
}

impl AddressSpaceLayout {
    /// Build a layout from the memory regions in a snapshot.
    ///
    /// Does not perform validation — call `validate()` afterwards.
    pub fn from_snapshot(snapshot: &ProcessSnapshot) -> Self {
        let mut ranges: Vec<AddrRange> = snapshot
            .memory_regions
            .iter()
            .map(|r| AddrRange {
                start: r.start_addr,
                end:   r.start_addr + r.size_bytes,
            })
            .collect();

        ranges.sort_by_key(|r| r.start);
        AddressSpaceLayout { ranges }
    }

    /// Verify the layout is restorable on this host.
    ///
    /// Checks:
    ///   - All addresses fit within the 47-bit user-space window.
    ///   - No two captured regions overlap (indicates a corrupted snapshot).
    ///   - At least one region exists (empty snapshots are rejected earlier).
    pub fn validate(&self) -> Result<()> {
        // Linux x86-64 user space ends at 2^47; kernel starts at 0xffff_8000_0000_0000.
        // If a region extends beyond this we cannot mmap it regardless of MAP_FIXED.
        const USER_SPACE_LIMIT: u64 = 1 << 47;

        for range in &self.ranges {
            if range.start >= USER_SPACE_LIMIT {
                return Err(anyhow!(
                    "Region at {:#x}–{:#x} starts above the 47-bit user-space boundary; \
                     cannot restore",
                    range.start, range.end
                ));
            }
            if range.end > USER_SPACE_LIMIT {
                return Err(anyhow!(
                    "Region {:#x}–{:#x} crosses the 47-bit user-space boundary",
                    range.start, range.end
                ));
            }
            if range.size() == 0 {
                return Err(anyhow!(
                    "Zero-size region at {:#x} in snapshot",
                    range.start
                ));
            }
        }

        // Detect overlaps (windows over sorted pairs).
        for pair in self.ranges.windows(2) {
            if pair[0].overlaps(&pair[1]) {
                return Err(anyhow!(
                    "Overlapping regions in snapshot: [{:#x},{:#x}) and [{:#x},{:#x})",
                    pair[0].start, pair[0].end,
                    pair[1].start, pair[1].end
                ));
            }
        }

        Ok(())
    }

    /// Return the highest address used by any region, rounded up to a page.
    ///
    /// Used to find a safe location above the snapshot's address space to place
    /// the restore stub — a region we control that won't conflict with anything
    /// we're about to mmap with MAP_FIXED.
    pub fn find_safe_base(&self) -> u64 {
        const PAGE: u64 = 4096;
        let max_end = self.ranges.iter().map(|r| r.end).max().unwrap_or(PAGE);
        // Round up to next page boundary.
        (max_end + PAGE - 1) & !(PAGE - 1)
    }

    /// Check whether a candidate address range conflicts with any snapshot region.
    ///
    /// Used to verify the restore stub address won't be clobbered by MAP_FIXED.
    pub fn conflicts_with(&self, candidate: &AddrRange) -> bool {
        self.ranges.iter().any(|r| r.overlaps(candidate))
    }

    /// Total bytes across all snapshot regions.
    pub fn total_bytes(&self) -> u64 {
        self.ranges.iter().map(|r| r.size()).sum()
    }
}

/// Decode a 4-character `perms` string (e.g. `"rwxp"`) into `PROT_*` flags.
///
/// Returns `libc::PROT_NONE` if no r/w/x bits are set.
/// The fourth character (private/shared) is ignored for mmap — we always use
/// MAP_PRIVATE | MAP_ANONYMOUS since we are reconstructing from raw bytes, not
/// a file-backed mapping.
pub fn perms_to_prot(perms: &str) -> i32 {
    let mut prot = libc::PROT_NONE;
    let bytes = perms.as_bytes();
    if bytes.first().copied() == Some(b'r') { prot |= libc::PROT_READ; }
    if bytes.get(1).copied()   == Some(b'w') { prot |= libc::PROT_WRITE; }
    if bytes.get(2).copied()   == Some(b'x') { prot |= libc::PROT_EXEC; }
    prot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_range_size() {
        let r = AddrRange { start: 0x1000, end: 0x3000 };
        assert_eq!(r.size(), 0x2000);
    }

    #[test]
    fn test_range_overlap() {
        let a = AddrRange { start: 0x1000, end: 0x3000 };
        let b = AddrRange { start: 0x2000, end: 0x4000 };
        let c = AddrRange { start: 0x3000, end: 0x5000 }; // adjacent, not overlapping
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c));
    }

    #[test]
    fn test_validate_overlap_detected() {
        let layout = AddressSpaceLayout {
            ranges: vec![
                AddrRange { start: 0x1000, end: 0x3000 },
                AddrRange { start: 0x2000, end: 0x4000 }, // overlaps
            ],
        };
        assert!(layout.validate().is_err());
    }

    #[test]
    fn test_validate_above_47bit_rejected() {
        let layout = AddressSpaceLayout {
            ranges: vec![AddrRange { start: 1 << 47, end: (1 << 47) + 0x1000 }],
        };
        assert!(layout.validate().is_err());
    }

    #[test]
    fn test_find_safe_base_rounded() {
        let layout = AddressSpaceLayout {
            ranges: vec![AddrRange { start: 0x1000, end: 0x2001 }],
        };
        let base = layout.find_safe_base();
        assert_eq!(base, 0x3000); // 0x2001 rounded up to next page
    }

    #[test]
    fn test_perms_to_prot() {
        assert_eq!(perms_to_prot("r--p"), libc::PROT_READ);
        assert_eq!(perms_to_prot("rw-p"), libc::PROT_READ | libc::PROT_WRITE);
        assert_eq!(perms_to_prot("r-xp"), libc::PROT_READ | libc::PROT_EXEC);
        assert_eq!(perms_to_prot("rwxp"), libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC);
        assert_eq!(perms_to_prot("---p"), libc::PROT_NONE);
    }
}
