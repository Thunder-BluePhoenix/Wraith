# Progress Tracker: Wraith Process Teleportation Engine

Last Updated: **2026-03-31** (Phase 5 in progress)

---

## Executive Summary

| Metric | Status | Notes |
|--------|--------|-------|
| **Overall Progress** | ~55% (Phase 5 in progress) | Capture + Transport + Restore done; Orchestration underway |
| **v1.0 ETA** | ~7 weeks remaining | Phases 5 + 6 + 7 on critical path |
| **Critical Path** | Phases 1→4→5→6 | Design + Capture + Restore must flow sequentially |

---

## Phase Status Dashboard

### Phase 1: Rust Foundation ✅ COMPLETE (code)
**Duration**: 2 weeks | **Owner**: Rust team

| Task | Status | Notes |
|------|--------|-------|
| Project structure | ✅ DONE | `wraith-rust/` created with all modules |
| Cargo.toml config | ✅ DONE | nix, prost, crc, clap, log (bincode removed — superseded by prost) |
| ptrace wrapper | ✅ DONE | `ProcessLock` with RAII attach/detach/drop |
| Register capture | ✅ DONE | x86-64 GP + FPU via PTRACE_GETREGS/GETFPREGS |
| Register validation | ✅ DONE | RIP range, RSP null, FPU size checks |
| Snapshot save/load | ✅ DONE | Protobuf via prost (replaced bincode in Phase 2) |
| CLI (capture/resume/inspect) | ✅ DONE | clap-based, 3 subcommands |
| Unit tests | ✅ DONE | Registers, utils, maps parser — graceful skip on non-Linux |
| Protobuf schema | ✅ DONE | `proto/wraith.proto` — shared by all phases |
| **Phase Gate** | ⚠ PENDING TEST | Compile + run on real Linux x86-64 with `ptrace_scope=0` |

**Deliverable**: `wraith-capturer` binary

**Risk**: ptrace permissions require `ptrace_scope=0` or root

---

### Phase 2: Memory Snapshot ✅ COMPLETE (code)
**Duration**: 3 weeks | **Owner**: Rust team (with Protocol team)

| Task | Status | Notes |
|------|--------|-------|
| Protobuf schema | ✅ DONE | `proto/wraith.proto` — all message types defined |
| build.rs / prost-build | ✅ DONE | Compiles proto at build time; no manual protoc needed |
| /proc/pid/maps parser | ✅ DONE | `memory.rs` — full parser with `classify_region` |
| /proc/pid/mem reader | ✅ DONE | `memory.rs` — `dump_region` + CRC-64/ECMA-182 checksum |
| Skip logic | ✅ DONE | Skips `[vsyscall]`, `[vvar]`, non-readable regions |
| Snapshot builder | ✅ DONE | `snapshot.rs` — `SnapshotBuilder` converts internal → proto types |
| FD enumeration | ✅ DONE | `fd_enum.rs` — type classify + fdinfo offset/flags |
| Capturer wired up | ✅ DONE | `capturer.rs` — full capture sequence: regs + memory + FDs |
| Protobuf save/load | ✅ DONE | `Capturer::save/load` via prost encode/decode |
| Integration tests | ✅ DONE | Proto roundtrip + live capture + FD enum tests |
| **Phase Gate** | ⚠ PENDING TEST | Validate snapshot.pb round-trip on real Linux |

**Deliverable**: `wraith-capturer` captures full process state (registers + memory + FDs) → `snapshot.pb`

**Dependency**: Phase 1 complete ✓

**Risk**: Memory read permissions on some systems (handled: fails gracefully, skips region)

---

### Phase 3: Go Transport ✅ COMPLETE (code)
**Duration**: 3 weeks | **Owner**: Go team

| Task | Status | Notes |
|------|--------|-------|
| Project setup | ✅ DONE | go.mod, Makefile, module `github.com/wraith/transfer` |
| Wire protocol | ✅ DONE | TCP framing: `[4B type][4B length][payload]`; control=JSON, data=binary |
| Delta detection | ✅ DONE | `pkg/delta/delta.go` — xxHash-64 page hashing, `Detector` struct |
| Transmitter (sender) | ✅ DONE | `pkg/transport/client.go` + `cmd/transmitter/main.go` |
| Receiver (listener) | ✅ DONE | `pkg/transport/server.go` + `cmd/receiver/main.go` |
| Per-block checksum verify | ✅ DONE | Receiver verifies xxHash on every `DataBlock` |
| Retry with backoff | ✅ DONE | Exponential backoff per block, configurable max retries |
| Integration tests | ✅ DONE | Delta, framing, DataBlock roundtrip, full e2e, large (10 MB) snapshot |
| **Phase Gate** | ⚠ PENDING TEST | `go test ./...` + real network transfer benchmark |

**Deliverable**: `wraith-transmitter` + `wraith-receiver` Go binaries

**Dependency**: Phase 2 Protobuf schema (Go transport treats snapshot as opaque bytes — no proto import needed)

**Risk**: Network timeout handling under packet loss

---

### Phase 4: Rust Restorer ✅ COMPLETE (code)
**Duration**: 3 weeks | **Owner**: Rust team (systems)

| Task | Status | Notes |
|------|--------|-------|
| Address space layout validation | ✅ DONE | `aslr.rs` — 47-bit boundary check, overlap detection, `perms_to_prot` |
| Child stub (fork + traceme) | ✅ DONE | `fork()` → child: `PTRACE_TRACEME` + `raise(SIGSTOP)` |
| Syscall injector | ✅ DONE | `SyscallInjector` — save regs / patch `syscall;int3` / restore |
| Virtual address mapping | ✅ DONE | Inject `mmap MAP_FIXED|MAP_PRIVATE|MAP_ANONYMOUS` per region |
| Memory restoration | ✅ DONE | Write pages via `/proc/pid/mem` (O_RDWR, child must be stopped) |
| Permission restoration | ✅ DONE | Inject `mprotect` after data write to restore final perms |
| Register restoration | ✅ DONE | `PTRACE_SETREGS` + `PTRACE_SETFPREGS` from proto register state |
| FD restoration | ✅ DONE | `fd_restore.rs` — regular + dir reopened; pipes/sockets warned |
| FD injection (dup2) | ✅ DONE | Inject `dup2` for each successfully opened FD |
| Checksum validation | ✅ DONE | CRC-64 verified before writing each region |
| CLI binary | ✅ DONE | `src/bin/restorer.rs` — `wraith-restorer --snapshot <path> [--strict-fds]` |
| Unit tests | ✅ DONE | `aslr`, `fd_restore`, `restorer` validate/prot tests |
| **Phase Gate** | ⚠ PENDING TEST | Restored process resumes without segfault on real Linux |

**Deliverable**: `wraith-restorer` binary

**Dependency**: Phase 2 (snapshot format), Phase 3 (network receive)

**Implementation**: ptrace syscall injection — no separate trampoline binary.
Parent forks stub child (PTRACE_TRACEME + SIGSTOP), injects mmap/mprotect/dup2, sets registers, detaches.

---

### Phase 5: Python Orchestration 🔄 IN PROGRESS
**Duration**: 2 weeks | **Owner**: Python team

| Task | Status | Notes |
|------|--------|-------|
| Project structure | ✅ DONE | `wraith-control/` — setup.py, requirements.txt, wraith/ package |
| Config dataclasses | ✅ DONE | `config.py` — DestinationConfig, CaptureConfig, TransferConfig, RestoreConfig |
| SSH session manager | ✅ DONE | `remote.py` — `RemoteSession` (paramiko), exec + file copy + port-forward |
| Exception types | ✅ DONE | `exceptions.py` — TeleportError, PreflightError, RollbackError, phased hierarchy |
| Logging setup | ✅ DONE | `logging.py` — structured JSON + human console handler |
| Pre-flight checks | ✅ DONE | `checks.py` — process exists, ptrace perms, arch match, dest reachable, resources, binaries |
| Teleporter state machine | ✅ DONE | `teleporter.py` — `MigrationState` enum, full migrate + rollback |
| CLI interface | ✅ DONE | `cli.py` — `migrate`, `capture`, `transfer`, `restore`, `check` subcommands |
| `__init__.py` public API | ✅ DONE | exports `Teleporter`, `DestinationConfig`, `TeleportError` |
| Unit tests | ✅ DONE | `tests/test_checks.py`, `tests/test_teleporter.py` |
| Example script | ✅ DONE | `examples/migrate_job.py` |
| **Phase Gate** | ⚠ PENDING TEST | Single workload migrates end-to-end on 2 machines |

**Deliverable**: `wraith` Python CLI tool + `wraith` importable library

**Dependency**: Phases 1–4 all working

**Risk**: SSH key management, timeouts on slow networks

---

### Phase 6: Full Integration Test ⚠ PLANNED
**Duration**: 2 weeks | **Owner**: QA / Integration

| Task | Status | Notes |
|------|--------|-------|
| Test environment setup | ⚠ PLANNED | 2 Linux VMs, `ptrace_scope=0`, Wraith binaries installed |
| Integration test suite | ⚠ PLANNED | pytest-based, uses real `wraith` CLI |
| Simple workload test | ⚠ PLANNED | Migrate a `sleep 3600` process |
| Memory preservation test | ⚠ PLANNED | Byte-for-byte comparison via snapshot inspect |
| Network failure test | ⚠ PLANNED | Kill connection mid-transfer; verify rollback unfreeze |
| Stress tests | ⚠ PLANNED | 10 sequential migrations, large processes (1 GB RSS) |
| Benchmark suite | ⚠ PLANNED | Measure pause time, transfer rate, restore time |
| **Phase Gate** | ⚠ NOT STARTED | All tests pass; pause < 30 s for typical workload |

**Deliverable**: Test suite + performance report

**Dependency**: All Phases 1–5

**Risk**: Finding reliable 2-machine test infrastructure

---

### Phase 7: Hardening ⚠ PLANNED
**Duration**: 3 weeks | **Owner**: Security + DevOps

| Task | Status | Notes |
|------|--------|-------|
| Error taxonomy | ⚠ PLANNED | Map all failure modes to error codes |
| Recovery strategies | ⚠ PLANNED | Retry vs abort vs rollback decision tree |
| Structured logging | ⚠ PLANNED | JSON logs with trace IDs across all components |
| Metrics export | ⚠ PLANNED | Prometheus counters: migrations, failures, bytes, latency |
| Security review | ⚠ PLANNED | Auth model, ptrace scope, snapshot file permissions |
| Playbooks | ⚠ PLANNED | Ops runbooks: frozen source, failed restore, partial transfer |
| Canary testing | ⚠ PLANNED | Gradual rollout plan for production |
| **Phase Gate** | ⚠ NOT STARTED | Security sign-off + runbooks reviewed |

**Deliverable**: Hardened binaries + runbooks + metrics dashboard

**Dependency**: Phases 1–6

**Risk**: Security issues found late; metrics instrumentation across 3 languages

---

### Phase 8: Beyond v1 ⚠ RESEARCH
**Duration**: Future | **Owner**: Architecture
**Status**: Do not start until v1.0 ships and validates in production

| Sub-phase | Status | Difficulty | Value |
|-----------|--------|-----------|-------|
| 8.1 Multi-threading | ⚠ PLANNED | High | Very High |
| 8.2 TCP socket live migration | ⚠ PLANNED | Very High | Very High |
| 8.3 Device handles | ⚠ PLANNED | Very High | Medium |
| 8.4 Cross-arch restore | ❌ UNLIKELY | Extreme | Low |
| 8.5 Live migration (no freeze) | ⚠ PLANNED | Research | High |
| 8.6 Container / namespace support | ⚠ PLANNED | Medium | High |
| 8.7 Observability + tracing | ⚠ PLANNED | Low | High |

---

## Critical Path Analysis

```
START
  │
  ├─ Phase 1: Rust Foundation    [✅ code done]
  │   └─ Phase 2: Memory Snapshot [✅ code done]
  │       └─ Phase 4: Rust Restorer [✅ code done]
  │           └─ Phase 5: Python Orchestration [🔄 IN PROGRESS ~2w]
  │               └─ Phase 6: Integration Tests [⚠ ~2w]
  │                   └─ Phase 7: Hardening [⚠ ~3w]
  │                       └─ v1.0 RELEASE
  │
  └─ Phase 3: Go Transport [✅ code done] (feeds into Phase 5)

Remaining on critical path: ~7 weeks
All phase gates pending real Linux test environment
```

---

## Milestone Tracking

### Milestone 1: Capture Working ✅ CODE DONE
- [x] Phase 1 code complete — ptrace, registers, CLI
- [x] Phase 2 code complete — maps, mem, FDs, proto
- [ ] Integration test passes on real Linux (phase gate pending)
- **Blocker for**: Phase 4

### Milestone 2: Transport Working ✅ CODE DONE
- [x] Phase 3 code complete — TCP framing, delta, transmitter/receiver
- [ ] Real network transfer test passes (phase gate pending)
- **Blocker for**: Phase 5

### Milestone 3: Single-Machine Restore ✅ CODE DONE
- [x] Phase 4 code complete — syscall injection, mmap, mprotect, setregs
- [ ] Restored process runs on real Linux without segfault (phase gate pending)
- **Blocker for**: End-to-end testing

### Milestone 4: First Cross-Machine Migration
- [ ] Phase 5 code complete (in progress)
- [ ] `wraith migrate --pid X --destination host-b` works on 2 real machines
- [ ] Rollback verified on simulated network failure
- **Blocker for**: Testing phase

### Milestone 5: Reliability
- [ ] Phase 6 tests all pass
- [ ] Stress tests hold (10 sequential migrations)
- [ ] Performance targets met (pause < 30 s for 1 GB process)
- **Blocker for**: Hardening

### Milestone 6: Production Ready
- [ ] Phase 7 hardening complete
- [ ] Security sign-off
- [ ] Canary test passed
- **Gate for**: v1.0 release

---

## Risk Register

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|-----------|
| ptrace permission issues on target systems | Medium | High | Require `ptrace_scope=0` or root; document in README |
| MAP_FIXED refused by kernel (ASLR conflict) | Medium | High | Detect early in `aslr.rs::validate()`; return clear error |
| Network timeout mid-transfer | Low | High | Retry with backoff (Phase 3); rollback unfreeze (Phase 5) |
| FD/socket restoration incomplete | High | Medium | v1 scope: accept loss of pipes/sockets; warn operator |
| Source left frozen on failure | High | Very High | RAII `ProcessLock` + Phase 5 rollback always unfreeze |
| Test environment unavailable | Medium | Very High | Acquire 2 Linux VMs before Phase 6 starts |
| SSH key management in production | Medium | Medium | Document key requirements; support agent forwarding |

---

## Success Criteria for v1.0

- [ ] Single-threaded processes migrate successfully (pending Phase 6 test)
- [ ] Memory state preserved byte-for-byte (pending Phase 6 verification)
- [ ] Source process never left frozen on any failure path (RAII guaranteed in code)
- [ ] Rollback works: source unfreezes if transfer or restore fails
- [ ] Pause time < 30 seconds for a 1 GB RSS process
- [ ] Clear error messages for all known failure cases
- [ ] `wraith migrate` CLI works end-to-end
- [ ] Documentation complete (docs/ all written)
- [ ] No known security issues (pending Phase 7 review)
- [ ] Benchmarks show acceptable performance (pending Phase 6)

---

## Known Issues (Tracking)

### Active (v1 Scope Limitations — by design)
- ❌ Multi-threaded processes not supported — `ptrace::attach` freezes one thread (Phase 8.1)
- ❌ Network sockets not preserved across machines — skip with warning (Phase 8.2)
- ❌ Device FDs (e.g. `/dev/tty`) not restored — skip with warning (Phase 8.3)
- ❌ Cross-architecture restore not supported — `compile_error!` guard in lib.rs (Phase 8.4)
- ⚠ MAP_FIXED may fail if destination kernel refuses exact address — returns error, rolls back
- ⚠ Anonymous pipe FDs lost on migrate — app must reopen; warning printed

### To Investigate Before Phase 6
- [ ] vsyscall / vdso restoration on kernels with `vsyscall=none`
- [ ] Large processes (>4 GB) — `/proc/pid/mem` seek uses `u64`; should be fine but test
- [ ] Processes with many threads detected early and rejected cleanly

---

## Repository Structure

```
wraith/
├── docs/                        (YOU ARE HERE)
│   ├── motto.md                ✓ Written
│   ├── plan.md                 ✓ Written
│   ├── roadmap.md              ✓ Written
│   ├── progress.md             ✓ This file
│   ├── phase1.md–phase8.md     ✓ All written
│
├── wraith-rust/                 ✓ Phases 1 + 2 + 4 complete
│   ├── Cargo.toml              ✓ nix, prost, crc, clap, log; [[bin]] for capturer + restorer
│   ├── build.rs                ✓ prost-build proto compilation
│   ├── proto/wraith.proto      ✓ Full schema (snapshot + transport messages)
│   ├── src/
│   │   ├── lib.rs              ✓ module declarations + platform guards
│   │   ├── main.rs             ✓ wraith-capturer CLI (capture / resume / inspect)
│   │   ├── bin/restorer.rs     ✓ wraith-restorer CLI (--snapshot + --strict-fds)
│   │   ├── proto.rs            ✓ prost-generated types
│   │   ├── capturer.rs         ✓ full capture: registers + memory + FDs
│   │   ├── ptrace_ops.rs       ✓ ProcessLock (RAII attach/detach/drop)
│   │   ├── registers.rs        ✓ Registers + from_ptrace + validate
│   │   ├── memory.rs           ✓ parse_maps + dump_region + CRC-64
│   │   ├── fd_enum.rs          ✓ FD type classification + fdinfo reader
│   │   ├── snapshot.rs         ✓ SnapshotBuilder (internal → proto)
│   │   ├── restorer.rs         ✓ ProcessRestorer + SyscallInjector
│   │   ├── aslr.rs             ✓ AddressSpaceLayout + perms_to_prot
│   │   ├── fd_restore.rs       ✓ FdRestorer (regular/dir ok; pipes/sockets warned)
│   │   ├── error.rs            ✓ anyhow re-exports + helpers
│   │   └── utils.rs            ✓ pid_exists, process_name, process_arch
│   └── tests/integration_tests.rs  ✓ proto roundtrip + live capture tests
│
├── wraith-go/                   ✓ Phase 3 complete
│   ├── go.mod                  ✓ module + xxhash dep
│   ├── Makefile                ✓ build / test / tidy
│   ├── pkg/transport/
│   │   ├── protocol.go         ✓ Frame, DataBlock, Conn, control messages
│   │   ├── client.go           ✓ Transmitter (send + delta + retry)
│   │   └── server.go           ✓ Receiver (checksum verify, buffer assembly)
│   ├── pkg/delta/delta.go      ✓ Detector, HashPage, Analyze
│   ├── cmd/transmitter/main.go ✓
│   ├── cmd/receiver/main.go    ✓
│   └── tests/integration_test.go  ✓ delta + framing + e2e roundtrip
│
├── wraith-control/              🔄 Phase 5 in progress
│   ├── setup.py                ✓ package metadata + entry points
│   ├── requirements.txt        ✓ click, paramiko, pytest
│   ├── wraith/
│   │   ├── __init__.py         ✓ public API exports
│   │   ├── config.py           ✓ DestinationConfig, CaptureConfig, etc.
│   │   ├── exceptions.py       ✓ TeleportError hierarchy
│   │   ├── logging.py          ✓ structured JSON + console handler
│   │   ├── remote.py           ✓ RemoteSession (paramiko SSH)
│   │   ├── checks.py           ✓ PreflightChecker (6 checks)
│   │   ├── teleporter.py       ✓ Teleporter state machine + rollback
│   │   └── cli.py              ✓ migrate / capture / transfer / restore / check
│   ├── tests/
│   │   ├── test_checks.py      ✓ unit tests for PreflightChecker
│   │   └── test_teleporter.py  ✓ state machine + rollback tests
│   └── examples/migrate_job.py ✓
│
└── README.md                    ✓ Written
```

---

## Next Steps

### Now (Phase 5 — in progress)
- [x] Python project skeleton created
- [x] All modules implemented
- [ ] Run `pip install -e .` and `pytest` locally (Linux)
- [ ] End-to-end smoke test against mock binaries

### After Phase 5 Gate
- [ ] Set up 2 Linux VMs with `ptrace_scope=0`
- [ ] Install Wraith binaries on both machines
- [ ] Run Phase 6 integration tests

### After Phase 6 Gate
- [ ] Phase 7 hardening — structured logging, Prometheus, security review

---

## Quick Reference

| What | Where | Owner |
|------|-------|-------|
| Big picture | [plan.md](plan.md) | Everyone |
| Philosophy | [motto.md](motto.md) | Leadership |
| Timeline | [roadmap.md](roadmap.md) | PM |
| This tracker | [progress.md](progress.md) | PM |
| Capture (Phase 1+2) | [phase1.md](phase1.md), [phase2.md](phase2.md) | Rust team |
| Transport (Phase 3) | [phase3.md](phase3.md) | Go team |
| Restore (Phase 4) | [phase4.md](phase4.md) | Rust team |
| Orchestration (Phase 5) | [phase5.md](phase5.md) | Python team |
| Testing (Phase 6) | [phase6.md](phase6.md) | QA |
| Hardening (Phase 7) | [phase7.md](phase7.md) | Security |
| Future (Phase 8) | [phase8.md](phase8.md) | Research |
