# Phase 2: Memory Snapshot — Parsing and Serialization

**Duration**: 3 weeks | **Owner**: Rust team (with Protocol team) | **Output**: Complete ProcessSnapshot proto implementation

## Goals

1. Parse `/proc/pid/maps` to enumerate all memory regions
2. Read actual bytes from `/proc/pid/mem` for each region
3. Serialize memory regions + register state via Protobuf
4. Validate snapshot integrity (checksums)
5. Support snapshot versioning

## Deliverables

### 2.1 Protobuf Schema

Create `wraith.proto`:

```protobuf
syntax = "proto3";

package wraith;

option go_package = "github.com/wraith/proto";
option java_package = "com.wraith.proto";

message ProcessSnapshot {
  uint32 pid = 1;
  uint32 uid = 2;
  string arch = 3;              // "x86_64"
  string kernel_version = 4;    // "5.15.0"
  uint64 captured_at_ns = 5;

  Registers registers = 10;
  repeated MemoryRegion memory_regions = 11;
  repeated FileDescriptor file_descriptors = 12;

  uint64 checksum = 20;         // CRC64 of entire snapshot
  string snapshot_version = 21; // "1.0"
}

message Registers {
  // General purpose registers (x86-64)
  uint64 rax = 1;   uint64 rbx = 2;   uint64 rcx = 3;   uint64 rdx = 4;
  uint64 rdi = 5;   uint64 rsi = 6;   uint64 rbp = 7;   uint64 rsp = 8;
  uint64 r8 = 9;    uint64 r9 = 10;   uint64 r10 = 11;  uint64 r11 = 12;
  uint64 r12 = 13;  uint64 r13 = 14;  uint64 r14 = 15;  uint64 r15 = 16;

  uint64 rip = 17;
  uint64 rflags = 18;

  // Floating point (FXSAVE format, 512 bytes)
  bytes fpu_state = 19;
}

message MemoryRegion {
  uint64 start_addr = 1;
  uint64 end_addr = 2;
  uint32 size_bytes = 3;

  // Permissions: "r", "w", "x", "p"/"s"
  string permissions = 4;

  // Type: "heap", "stack", "vdso", "vsyscall", "file", "mmap", "anon"
  string region_type = 5;

  // If file-backed
  string backing_file = 6;  // "/path/to/file" or empty
  uint64 file_offset = 7;
  
  // Actual page data (compressed optional in v2)
  bytes data = 8;

  uint64 checksum = 9;      // CRC64 of data
}

message FileDescriptor {
  uint32 fd_num = 1;
  string fd_type = 2;        // "regular", "pipe", "socket", "device"
  string path = 3;           // For "regular": path to file; for others: dev info
  uint64 file_offset = 4;    // Current seek position
  uint32 flags = 5;          // O_RDONLY, O_WRONLY, etc.
}

message SnapshotMetadata {
  string machine_hostname = 1;
  string process_name = 2;
  uint64 process_age_seconds = 3;
  uint32 thread_count = 4;    // v1: always 1
}
```

### 2.2 Rust Implementation

#### **memory.rs** — Memory region handling
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub perms: String,      // "rwxp"
    pub backing_file: Option<String>,
    pub offset: u64,
    pub data: Vec<u8>,
}

pub struct MemoryDumper;

impl MemoryDumper {
    /// Parse /proc/<pid>/maps and return all regions
    pub fn parse_maps_file(pid: i32) -> Result<Vec<MemoryRegion>> {
        let maps = std::fs::read_to_string(format!("/proc/{}/maps", pid))?;
        let mut regions = Vec::new();

        for line in maps.lines() {
            // Parse: address perms offset dev inode pathname
            // E.g. 7f1234567000-7f1234568000 r--p 1000 08:01 1234 /lib/libc.so.6
            
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 5 {
                continue;
            }

            let addresses = parts[0].split('-').collect::<Vec<_>>();
            let start = u64::from_str_radix(addresses[0], 16)?;
            let end = u64::from_str_radix(addresses[1], 16)?;
            let perms = parts[1];

            let backing_file = if parts.len() > 5 && !parts[5].starts_with('[') {
                Some(parts[5].to_string())
            } else {
                None
            };

            regions.push(MemoryRegion {
                start,
                end,
                perms: perms.to_string(),
                backing_file,
                offset: 0,  // Could parse from /proc/pid/cmdline if needed
                data: Vec::new(),  // Populated next
            });
        }

        Ok(regions)
    }

    /// Read actual memory from /proc/<pid>/mem
    pub fn dump_memory(pid: i32, region: &MemoryRegion) -> Result<Vec<u8>> {
        let mut mem_file = std::fs::File::open(format!("/proc/{}/mem", pid))?;
        mem_file.seek(SeekFrom::Start(region.start))?;

        let size = (region.end - region.start) as usize;
        let mut buffer = vec![0u8; size];
        mem_file.read_exact(&mut buffer)?;

        Ok(buffer)
    }

    /// Calculate CRC-64 checksum
    pub fn checksum_data(data: &[u8]) -> u64 {
        use crc::{Crc, CRC_64_ECMA};
        let crc = Crc::<u64>::new(&CRC_64_ECMA);
        let mut digest = crc.digest();
        digest.update(data);
        digest.finalize()
    }
}
```

#### **snapshot.rs** — Snapshot creation and serialization
```rust
pub struct SnapshotBuilder {
    registers: Option<Registers>,
    regions: Vec<MemoryRegion>,
    fds: Vec<FileDescriptor>,
}

impl SnapshotBuilder {
    pub fn new() -> Self {
        SnapshotBuilder {
            registers: None,
            regions: Vec::new(),
            fds: Vec::new(),
        }
    }

    pub fn with_registers(mut self, regs: Registers) -> Self {
        self.registers = Some(regs);
        self
    }

    pub fn add_memory_region(mut self, region: MemoryRegion) -> Self {
        self.regions.push(region);
        self
    }

    pub fn build(self) -> Result<ProcessSnapshot> {
        let registers = self.registers.ok_or_else(|| anyhow!("No registers set"))?;

        Ok(ProcessSnapshot {
            registers,
            memory_regions: self.regions,
            file_descriptors: self.fds,
            // ... populate metadata
        })
    }

    pub fn to_protobuf(&self) -> Result<Vec<u8>> {
        let snapshot_proto = wraith_pb::ProcessSnapshot {
            // Convert to protobuf format
        };
        Ok(snapshot_proto.encode_to_vec())
    }

    pub fn to_file(&self, path: &str) -> Result<()> {
        let data = self.to_protobuf()?;
        std::fs::write(path, data)?;
        Ok(())
    }
}
```

#### **capturer.rs** — Full integration
```rust
pub struct Capturer {
    pid: i32,
}

impl Capturer {
    pub fn capture(pid: i32) -> Result<ProcessSnapshot> {
        let process = ProcessLock::attach(pid)?;

        let mut builder = SnapshotBuilder::new();

        // 1. Get registers
        let registers = process.get_registers()?;
        builder = builder.with_registers(registers);

        // 2. Enumerate memory regions
        let regions = MemoryDumper::parse_maps_file(pid)?;
        for mut region in regions {
            // Skip certain regions (vsyscall, etc)
            if Self::should_skip_region(&region) {
                continue;
            }

            // Read memory
            region.data = MemoryDumper::dump_memory(pid, &region)?;
            region.checksum = MemoryDumper::checksum_data(&region.data);

            builder = builder.add_memory_region(region);
        }

        // 3. Enumerate FDs (stub for now)
        // In Phase 3, enumerate /proc/pid/fd

        process.detach()?;
        builder.build()
    }

    fn should_skip_region(region: &MemoryRegion) -> bool {
        // Skip [vsyscall], [vvar], etc.
        // Keep only real data regions for v1
        region.backing_file.as_ref()
            .map(|f| f.starts_with("["))
            .unwrap_or(false)
    }
}
```

### 2.3 File Descriptor Enumeration

Create **fd_enum.rs**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDescriptor {
    pub fd: u32,
    pub ty: FileDescriptorType,
    pub path: String,
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileDescriptorType {
    Regular,      // Regular file
    Pipe,         // Pipe
    Socket,       // Socket (phase 7+)
    Device,       // Device (phase 7+)
    Directory,    // Directory (rare to have open)
}

pub fn enumerate_fds(pid: i32) -> Result<Vec<FileDescriptor>> {
    let fd_dir = std::fs::read_dir(format!("/proc/{}/fd", pid))?;
    let mut fds = Vec::new();

    for entry in fd_dir {
        let entry = entry?;
        let path = entry.path();
        let fd_num = path.file_name()
            .and_then(|n| n.to_str())
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| anyhow!("Invalid FD"))?;

        let target = std::fs::read_link(&path)?;
        let ty = match target.to_str().unwrap_or("") {
            s if s.starts_with("socket:") => FileDescriptorType::Socket,
            s if s.starts_with("pipe:") => FileDescriptorType::Pipe,
            _ => FileDescriptorType::Regular,
        };

        fds.push(FileDescriptor {
            fd: fd_num,
            ty,
            path: target.to_string_lossy().to_string(),
            offset: 0,  // TODO: read from /proc/pid/fdinfo
        });
    }

    Ok(fds)
}
```

## Testing Strategy

### Unit Tests
- Protobuf serialization/deserialization
- Memory region parsing from `/proc/<pid>/maps`
- Checksum validation
- Snapshot versioning

### Integration Tests
- Capture full process: registers + all memory
- Dump to file, reload, verify identical
- Memory integrity (checksums match)

**Test target**: Process that allocates heap, uses stack, loads libraries
```c
// test_mem.c
#include <stdlib.h>
#include <string.h>
int main() {
    char* heap = malloc(1000000);
    strcpy(heap, "test");
    while(1) {
        memset(heap, 0, 100);
    }
    return 0;
}
```

## Validation Checklist

- [ ] `/proc/pid/maps` parsed correctly
- [ ] Memory reads from `/proc/pid/mem` succeed
- [ ] Checksums computed and verified
- [ ] Protobuf encode/decode works
- [ ] Snapshot file size reasonable
- [ ] Multiple runs produce identical snapshots (deterministic)
- [ ] File descriptors enumerated without errors

## Known Limitations (v1 only)

- ❌ Sparse memory regions not optimized (Phase 3 delta transfer handles this)
- ❌ Device files skipped
- ❌ Sockets captured as FD list only, state not preserved (Phase 7)
- ✓ All regular memory captured byte-for-byte

## Dependencies

- **Phase 1**: Registers + ProcessLock
- **Phase 3**: Delta transfer (reduces snapshot size)
- **Phase 4**: Memory restore uses this schema

## Success Criteria

- [x] Full memory dump captures
- [x] Snapshot file generated
- [x] Protobuf validates
- [x] Integration test passes
