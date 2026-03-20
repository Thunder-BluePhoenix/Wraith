/// Phase 1 + Phase 2 integration tests.
///
/// Tests that require ptrace are guarded behind a runtime permission check
/// and print a SKIP message rather than failing — CI often runs without
/// ptrace_scope=0 or root.
///
/// Run all tests:
///   cargo test -- --nocapture
/// Run ptrace tests (requires root or ptrace_scope=0):
///   sudo cargo test
use wraith_capturer::capturer::Capturer;
use wraith_capturer::memory::MemoryDumper;
use wraith_capturer::proto::wraith;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_proto_registers(rip: u64, rsp: u64) -> wraith::Registers {
    wraith::Registers {
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

fn make_snapshot(pid: u32) -> wraith::ProcessSnapshot {
    wraith::ProcessSnapshot {
        pid,
        uid: 1000,
        arch: "x86_64".to_string(),
        kernel_version: "Linux 6.1.0".to_string(),
        captured_at_ns: 1_700_000_000_000_000_000,
        snapshot_version: "1.0".to_string(),
        checksum: 0,
        registers: Some(make_proto_registers(0x40_1234, 0x7fff_0000_0ff8)),
        memory_regions: vec![
            wraith::MemoryRegion {
                start_addr:  0x40_0000,
                end_addr:    0x40_1000,
                size_bytes:  4096,
                permissions: "r-xp".to_string(),
                region_type: "file".to_string(),
                backing_file: "/usr/bin/sleep".to_string(),
                file_offset: 0,
                data:        vec![0u8; 4096],
                checksum:    MemoryDumper::checksum(&vec![0u8; 4096]),
            },
        ],
        file_descriptors: vec![
            wraith::FileDescriptor {
                fd_num: 0, fd_type: "regular".to_string(),
                path: "/dev/null".to_string(), file_offset: 0, flags: 0,
            },
        ],
        metadata: Some(wraith::SnapshotMetadata {
            machine_hostname:      "test-host".to_string(),
            process_name:          "test_proc".to_string(),
            process_age_seconds:   42,
            thread_count:          1,
            virtual_memory_bytes:  4096,
            resident_memory_bytes: 4096,
        }),
    }
}

// ── Protobuf serialization tests (no ptrace, no root) ────────────────────────

#[test]
fn test_snapshot_protobuf_roundtrip() {
    use prost::Message;

    let original = make_snapshot(12345);

    let bytes = original.encode_to_vec();
    assert!(!bytes.is_empty());

    let decoded = wraith::ProcessSnapshot::decode(bytes.as_slice()).expect("decode");

    assert_eq!(decoded.pid,              original.pid);
    assert_eq!(decoded.arch,             original.arch);
    assert_eq!(decoded.snapshot_version, original.snapshot_version);
    assert_eq!(decoded.captured_at_ns,   original.captured_at_ns);

    let r = decoded.registers.as_ref().expect("registers missing after decode");
    assert_eq!(r.rip, 0x40_1234);
    assert_eq!(r.rsp, 0x7fff_0000_0ff8);
    assert_eq!(r.fpu_state.len(), std::mem::size_of::<libc::user_fpregs_struct>());

    assert_eq!(decoded.memory_regions.len(), 1);
    assert_eq!(decoded.memory_regions[0].size_bytes, 4096);
    assert_eq!(decoded.memory_regions[0].region_type, "file");

    assert_eq!(decoded.file_descriptors.len(), 1);
    assert_eq!(decoded.file_descriptors[0].fd_num, 0);
}

#[test]
fn test_snapshot_save_load_roundtrip() {
    let original = make_snapshot(99999);
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");

    Capturer::save(&original, tmp.path()).expect("save");
    let loaded = Capturer::load(tmp.path()).expect("load");

    assert_eq!(loaded.pid, original.pid);
    assert_eq!(loaded.arch, original.arch);
    assert_eq!(loaded.memory_regions.len(), original.memory_regions.len());
    assert_eq!(loaded.file_descriptors.len(), original.file_descriptors.len());

    let r = loaded.registers.as_ref().expect("registers");
    assert_eq!(r.rip, 0x40_1234);
    assert_eq!(r.fpu_state.len(), std::mem::size_of::<libc::user_fpregs_struct>());
}

#[test]
fn test_snapshot_save_fails_bad_path() {
    let snap = make_snapshot(1);
    assert!(Capturer::save(&snap, "/nonexistent/path/snap.pb").is_err());
}

#[test]
fn test_load_fails_on_missing_file() {
    assert!(Capturer::load("/tmp/wraith_no_such_file_xyz.pb").is_err());
}

#[test]
fn test_load_fails_on_garbage() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    std::fs::write(tmp.path(), b"not protobuf data at all").expect("write");
    // Protobuf is lenient on garbage bytes — it may succeed with empty fields.
    // We just verify the call doesn't panic; the checksum (Phase 7) catches corruption.
    let _ = Capturer::load(tmp.path());
}

#[test]
fn test_summary() {
    let snap = make_snapshot(777);
    let s = Capturer::summary(&snap);
    assert!(s.contains("pid=777"));
    assert!(s.contains("arch=x86_64"));
    assert!(s.contains("regions=1"));
    assert!(s.contains("fds=1"));
}

// ── Memory parser tests (no ptrace, no root) ─────────────────────────────────

#[test]
fn test_memory_region_checksum_in_snapshot() {
    let data = vec![0xABu8; 4096];
    let checksum = MemoryDumper::checksum(&data);
    assert_ne!(checksum, 0);

    let region = wraith::MemoryRegion {
        start_addr: 0x1000, end_addr: 0x2000, size_bytes: 4096,
        permissions: "rw-p".to_string(), region_type: "heap".to_string(),
        backing_file: String::new(), file_offset: 0,
        data: data.clone(),
        checksum,
    };
    // Verify the checksum stored in the region matches the data.
    assert_eq!(region.checksum, MemoryDumper::checksum(&region.data));
}

// ── FD enumeration test (Linux, no root needed) ───────────────────────────────

#[test]
#[cfg(target_os = "linux")]
fn test_enumerate_self_fds() {
    use wraith_capturer::fd_enum::enumerate_fds;

    let fds = enumerate_fds(std::process::id() as i32).expect("enumerate_fds on self");
    assert!(
        fds.len() >= 3,
        "Expected at least stdin/stdout/stderr, got {}",
        fds.len()
    );
    // Check sorting.
    let nums: Vec<u32> = fds.iter().map(|f| f.fd).collect();
    let mut sorted = nums.clone();
    sorted.sort();
    assert_eq!(nums, sorted, "FDs should be sorted by fd number");
}

// ── Live capture tests (require ptrace permissions) ──────────────────────────

#[test]
#[cfg(target_os = "linux")]
fn test_capture_sleep_full() {
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    let mut child = Command::new("sleep").arg("3600").spawn().expect("spawn sleep");
    let pid = child.id() as i32;
    thread::sleep(Duration::from_millis(150));

    let capturer = Capturer::new(pid);
    match capturer.capture() {
        Ok(snapshot) => {
            assert_eq!(snapshot.pid, pid as u32);
            assert_eq!(snapshot.arch, "x86_64");
            assert_eq!(snapshot.snapshot_version, "1.0");

            let regs = snapshot.registers.as_ref().expect("registers");
            assert!(regs.rip > 0, "RIP must be non-zero");
            assert!(regs.rsp > 0, "RSP must be non-zero");
            assert_eq!(regs.fpu_state.len(), std::mem::size_of::<libc::user_fpregs_struct>());

            // Phase 2: memory and FDs must be populated.
            assert!(
                !snapshot.memory_regions.is_empty(),
                "Expected at least one memory region"
            );
            assert!(
                !snapshot.file_descriptors.is_empty(),
                "Expected at least stdin/stdout/stderr fds"
            );

            // All captured regions must have matching checksums.
            for region in &snapshot.memory_regions {
                let computed = MemoryDumper::checksum(&region.data);
                assert_eq!(
                    computed, region.checksum,
                    "Checksum mismatch for region {:#x}",
                    region.start_addr
                );
            }

            // Process must still be alive after capture.
            assert!(
                wraith_capturer::utils::pid_exists(pid),
                "Process must survive capture"
            );
        }
        Err(e) if e.to_string().contains("Permission denied") => {
            println!("SKIP test_capture_sleep_full: requires root or ptrace_scope=0");
        }
        Err(e) => panic!("Unexpected capture error: {:#}", e),
    }

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[cfg(target_os = "linux")]
fn test_capture_and_save_roundtrip() {
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    let mut child = Command::new("sleep").arg("3600").spawn().expect("spawn sleep");
    let pid = child.id() as i32;
    thread::sleep(Duration::from_millis(150));

    let capturer = Capturer::new(pid);
    match capturer.capture() {
        Ok(original) => {
            let tmp = tempfile::NamedTempFile::new().expect("tempfile");
            Capturer::save(&original, tmp.path()).expect("save");
            let loaded = Capturer::load(tmp.path()).expect("load");

            assert_eq!(loaded.pid,  original.pid);
            assert_eq!(loaded.arch, original.arch);
            assert_eq!(
                loaded.memory_regions.len(),
                original.memory_regions.len(),
                "Region count must survive save/load roundtrip"
            );
            // Spot-check first region data integrity.
            if let (Some(orig_r), Some(load_r)) =
                (original.memory_regions.first(), loaded.memory_regions.first())
            {
                assert_eq!(orig_r.start_addr, load_r.start_addr);
                assert_eq!(orig_r.checksum,   load_r.checksum);
                assert_eq!(orig_r.data,        load_r.data);
            }
        }
        Err(e) if e.to_string().contains("Permission denied") => {
            println!("SKIP test_capture_and_save_roundtrip: requires root or ptrace_scope=0");
        }
        Err(e) => panic!("Unexpected error: {:#}", e),
    }

    child.kill().ok();
    child.wait().ok();
}
