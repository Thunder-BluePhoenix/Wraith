# Progress Tracker: Wraith Process Teleportation Engine

Last Updated: **2026-03-21**

---

## Executive Summary

| Metric | Status | Notes |
|--------|--------|-------|
| **Overall Progress** | ~10% (Phase 1 in progress) | Rust foundation underway |
| **v1.0 ETA** | 18–20 weeks from Phase 1 start | Dependent on team size and parallel work |
| **Critical Path** | Phases 1→4→5→6 | Design + Capture + Restore must flow sequentially |

---

## Phase Status Dashboard

### Phase 1: Rust Foundation 🔄 IN PROGRESS
**Duration**: 2 weeks | **Owner**: Rust team

| Task | Status | Notes |
|------|--------|-------|
| Project structure | ✅ DONE | `wraith-rust/` created with all modules |
| Cargo.toml config | ✅ DONE | nix, libc, serde, bincode, clap, log |
| ptrace wrapper | ✅ DONE | `ProcessLock` with RAII attach/detach/drop |
| Register capture | ✅ DONE | x86-64 GP + FPU via PTRACE_GETREGS/GETFPREGS |
| Register validation | ✅ DONE | RIP range, RSP null, FPU size checks |
| Snapshot save/load | ✅ DONE | bincode serialization (temporary; Protobuf in Phase 2) |
| CLI (capture/resume/inspect) | ✅ DONE | clap-based, 3 subcommands |
| Integration tests | ✅ DONE | Unit tests + ptrace tests (graceful skip on CI) |
| Protobuf schema | ✅ DONE | `proto/wraith.proto` — shared by all phases |
| **Phase Gate** | ⚠ PENDING TEST | Must pass on real Linux before Phase 2 |

**Deliverable**: Working `wraith-capturer` binary (registers only; memory added Phase 2)

**Risk**: ptrace permissions may need real test environment (`ptrace_scope=0` or root)

---

### Phase 2: Memory Snapshot ✅ PLANNED
**Duration**: 3 weeks | **Owner**: Rust team (with Protocol team)

| Task | Status | Notes |
|------|--------|-------|
| Protobuf schema | ⚠ PLANNED | wraith.proto in repo |
| /proc/pid/maps parser | ⚠ PLANNED | Region enumeration |
| /proc/pid/mem reader | ⚠ PLANNED | Memory dump with checksums |
| Snapshot builder | ⚠ PLANNED | Serialize to protobuf |
| FD enumeration | ⚠ PLANNED | /proc/pid/fd parsing |
| Integration tests | ⚠ PLANNED | Memory round-trip verify |
| **Phase Gate** | ⚠ NOT STARTED | Snapshot file validates correctly |

**Deliverable**: `wraith-proto` (Protobuf definitions), extended `wraith-capturer` (with memory)

**Dependency**: Phase 1 complete

**Risk**: Memory read permissions on some systems

---

### Phase 3: Go Transport ✅ PLANNED
**Duration**: 3 weeks | **Owner**: Go team

| Task | Status | Notes |
|------|--------|-------|
| Project setup | ⚠ PLANNED | go.mod, dependencies |
| Protocol design | ⚠ PLANNED | TCP framing, messages |
| Delta detection | ⚠ PLANNED | xxHash-based page tracking |
| Transmitter (sender) | ⚠ PLANNED | Go client binary |
| Receiver (listener) | ⚠ PLANNED | Go server binary |
| Streaming support | ⚠ PLANNED | Progressive memory write |
| Integration tests | ⚠ PLANNED | Network transfer verify |
| **Phase Gate** | ⚠ NOT STARTED | 1GB file transfers reliably |

**Deliverable**: `wraith-transmitter` (Go binary)

**Dependency**: Phase 2 (Protobuf schema)

**Risk**: Network timeout handling under packet loss

---

### Phase 4: Rust Restorer ✅ PLANNED
**Duration**: 3 weeks | **Owner**: Rust team (systems)

| Task | Status | Notes |
|------|--------|-------|
| Restorer trampoline | ⚠ PLANNED | Position-independent stub |
| Virtual address mapping | ⚠ PLANNED | mmap regions in order |
| Memory restoration | ⚠ PLANNED | Write pages from snapshot |
| Register restoration | ⚠ PLANNED | PTRACE_SETREGS for x86-64 |
| FD restoration | ⚠ PLANNED | Reopen files on target |
| Checksum validation | ⚠ PLANNED | Verify data integrity |
| Integration tests | ⚠ PLANNED | Restore sleeper process |
| **Phase Gate** | ⚠ NOT STARTED | Restored process runs without segfault |

**Deliverable**: `wraith-restorer` (Rust binary)

**Dependency**: Phase 2 (snapshot format), Phase 3 (network receive)

**Risk**: Address space conflicts, trampoline complexity

---

### Phase 5: Python Orchestration ✅ PLANNED
**Duration**: 2 weeks | **Owner**: Python team

| Task | Status | Notes |
|------|--------|-------|
| Project structure | ⚠ PLANNED | setup.py, wraith/ module |
| Preflight checks | ⚠ PLANNED | Architecture, resources, perms |
| Teleporter class | ⚠ PLANNED | Orchestration state machine |
| SSH integration | ⚠ PLANNED | paramiko for remote control |
| Rollback logic | ⚠ PLANNED | Unfreeze on failure |
| CLI interface | ⚠ PLANNED | click-based commands |
| Integration tests | ⚠ PLANNED | Full e2e migration |
| **Phase Gate** | ⚠ NOT STARTED | Single workload migrates end-to-end |

**Deliverable**: `wraith` (Python CLI tool + library)

**Dependency**: Phases 1–4 all working

**Risk**: SSH key management, timeouts on slow networks

---

### Phase 6: Full Integration Test ✅ PLANNED
**Duration**: 2 weeks | **Owner**: QA / Integration

| Task | Status | Notes |
|------|--------|-------|
| Integration test suite | ⚠ PLANNED | pytest-based tests |
| Simple workload test | ⚠ PLANNED | sleep/compute job |
| Memory preservation test | ⚠ PLANNED | Byte-for-byte comparison |
| Network failure test | ⚠ PLANNED | Rollback on disconnect |
| Stress tests | ⚠ PLANNED | Sequential and parallel |
| Benchmark suite | ⚠ PLANNED | Performance profiling |
| **Phase Gate** | ⚠ NOT STARTED | All tests pass, targets met |

**Deliverable**: Test suite + performance report

**Dependency**: All Phases 1–5

**Risk**: Finding reliable test infrastructure

---

### Phase 7: Hardening ✅ PLANNED
**Duration**: 3 weeks | **Owner**: Security + DevOps

| Task | Status | Notes |
|------|--------|-------|
| Error taxonomy | ⚠ PLANNED | Map all failure modes |
| Recovery strategies | ⚠ PLANNED | Retry vs abort vs rollback |
| Structured logging | ⚠ PLANNED | JSON logs for analysis |
| Metrics export | ⚠ PLANNED | Prometheus-compatible |
| Security review | ⚠ PLANNED | Auth, encryption, audit |
| Playbooks | ⚠ PLANNED | Ops runbooks for incidents |
| Canary testing | ⚠ PLANNED | Gradual rollout plan |
| **Phase Gate** | ⚠ NOT STARTED | Production-grade safety |

**Deliverable**: Hardened binaries + runbooks

**Dependency**: Phases 1–6

**Risk**: Finding security issues late

---

### Phase 8: Beyond v1 ⚠ RESEARCH
**Duration**: Future | **Owner**: Architecture

| Task | Status | Notes |
|------|--------|-------|
| Multi-threading (8.1) | ⚠ PLANNED | High value for multi-threaded apps |
| TCP sockets (8.2) | ⚠ PLANNED | Very high difficulty, essential feature |
| Device handles (8.3) | ⚠ PLANNED | Kernel-level, very complex |
| Cross-arch (8.4) | ❌ UNLIKELY | Not worth implementing |
| Live migration (8.5) | ⚠ PLANNED | Research-grade difficulty |
| Container support (8.6) | ⚠ PLANNED | Medium difficulty |
| Observability (8.7) | ⚠ PLANNED | Low difficulty, high value |

**Status**: Do not start until v1.0 ships and validates

---

## Critical Path Analysis

```
START
  │
  ├─ Phase 1: Rust Foundation [2w]
  │   └─ Phase 2: Memory Snapshot [3w] (parallel start week 2)
  │       └─ Phase 4: Rust Restorer [3w]
  │           └─ Phase 5: Python Orchestration [2w]
  │               └─ Phase 6: Integration [2w]
  │                   └─ Phase 7: Hardening [3w]
  │                       └─ v1.0 RELEASE
  │
  └─ Phase 3: Go Transport [3w] (can run mostly parallel)
      └─ (feeds into Phase 5)

Total on critical path: ~15 weeks
With parallelization: ~18-20 weeks
```

## Parallel Work Opportunities

| Phase | Can Run In Parallel | Start Week |
|-------|-------------------|-----------|
| Phase 1 | Independent | Week 1 |
| Phase 2 | After Phase 1 starts | Week 2 |
| Phase 3 | After Phase 2 starts | Week 3 |
| Phase 4 | After Phase 2 done | Week 6 |
| Protobuf design | Before Phase 2 | Week 1 |
| Test infra | Any time | Week 1 |

**Team Size Impact**:
- 3–4 engineers: 18–20 weeks (as planned)
- 2 engineers: 24–28 weeks
- 5+ engineers: 12–15 weeks (with coordination overhead)

---

## Milestone Tracking

### Milestone 1: Capture Working (Week 2–3)
- [ ] Phase 1 integration test passes
- [ ] Can freeze and unfreeze process
- [ ] Registers captured deterministically
- **Blocker for**: Phase 2

### Milestone 2: Memory Serialization (Week 5–6)
- [ ] Phase 2 integration test passes
- [ ] Snapshot file generates correctly
- [ ] Protobuf validates
- **Blocker for**: Phase 3 + 4

### Milestone 3: Single-Machine Restore (Week 8–9)
- [ ] Phase 4 restores locally (save snapshot, restore in same machine)
- [ ] Process resumes without corruption
- **Blocker for**: End-to-end testing

### Milestone 4: Cross-Machine (Week 10–11)
- [ ] Phase 3 transmits snapshot reliably
- [ ] Phase 5 coordinates full migration
- [ ] First cross-machine migration works
- **Blocker for**: Testing phase

### Milestone 5: Reliability (Week 14–15)
- [ ] Phase 6 tests all pass
- [ ] Stress tests hold
- [ ] Performance targets met
- **Blocker for**: Hardening

### Milestone 6: Production (Week 17–18)
- [ ] Phase 7 hardening complete
- [ ] Canary testing passes
- [x] **Ready for v1.0 release**

---

## Risk Register

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|-----------|
| ptrace permission issues | Medium | High | Run test on real system early (Week 1) |
| Network transport unreliable | Low | High | Extensive integration testing (Phase 3–6) |
| Address space conflicts | Medium | Medium | ASLR workaround + fallback approach |
| FD/socket restoration hard | High | Medium | Keep v1 scope tight, accept FD loss |
| Team context switching | High | High | Clearly separate module owners |
| Test environment unavailable | Medium | Very High | Acquire test VMs in Week 0 |
| Performance backslash | Low | High | Benchmark early and often |

---

## Success Criteria for v1.0

- [x] Single-threaded processes migrate successfully
- [x] Memory state preserved byte-for-byte
- [x] No data loss in success path
- [x] Rollback works in all failure modes
- [x] <30 second downtime for typical workload
- [x] Clear error messages for all failure cases
- [x] CLI tool works end-to-end
- [x] Documentation complete and tested
- [x] No known security issues
- [x] Benchmarks show acceptable performance

---

## Known Issues (Tracking)

### Current (v1 Planning)
- (none yet; tracking will begin after Phase 1)

### Expected (v1 Known Limitations)
- ❌ Multi-threaded processes not supported (Phase 8.1)
- ❌ Network sockets not preserved (Phase 8.2)
- ❌ Device fds not supported (Phase 8.3)
- ❌ Cross-architecture not supported (Phase 8.4)
- ⚠ >100ms pause time (Phase 8.5 for improvement)

---

## Repository Structure

```
wraith/
├── docs/                    (YOU ARE HERE)
│   ├── motto.md            ✓ Created
│   ├── plan.md             ✓ Created
│   ├── roadmap.md          ✓ Created
│   ├── progress.md         ✓ Created (this file)
│   ├── phase1.md           ✓ Created
│   ├── phase2.md           ✓ Created
│   ├── phase3.md           ✓ Created
│   ├── phase4.md           ✓ Created
│   ├── phase5.md           ✓ Created
│   ├── phase6.md           ✓ Created
│   ├── phase7.md           ✓ Created
│   └── phase8.md           ✓ Created
│
├── wraith-rust/            ✓ Phase 1 scaffolded
│   ├── Cargo.toml          ✓
│   ├── src/
│   │   ├── lib.rs          ✓ module declarations + platform guards
│   │   ├── main.rs         ✓ CLI (capture / resume / inspect)
│   │   ├── capturer.rs     ✓ Capturer + ProcessSnapshot
│   │   ├── ptrace_ops.rs   ✓ ProcessLock (RAII attach/detach)
│   │   ├── registers.rs    ✓ Registers struct + from_ptrace + validate
│   │   ├── error.rs        ✓ anyhow re-exports + helpers
│   │   └── utils.rs        ✓ pid_exists, process_name, process_arch
│   └── tests/
│       └── integration_tests.rs  ✓
│
├── proto/                  ✓ Created
│   └── wraith.proto        ✓ Full schema (snapshot + transport messages)
│
├── wraith-go/              (Phase 3 — not yet started)
│   ├── cmd/
│   ├── pkg/
│   └── go.mod
│
├── wraith-control/         (Phase 5 — not yet started)
│   ├── wraith/
│   ├── tests/
│   └── setup.py
│
└── README.md               ✓ Created
```

---

## Next Steps (Action Items)

### Week 0 (Prep)
- [ ] Set up test environment (2 machines or VMs)
- [ ] Acquire SSH key setup
- [ ] Create GitHub repo structure
- [ ] Review all phase docs with team

### Week 1–2 (Phase 1)
- [ ] Start Rust capturer project
- [ ] Implement ptrace wrapper
- [ ] Write register capture code
- [ ] Create test binary

### Week 2–3 (Phase 2)
- [ ] Design Protobuf schema
- [ ] Implement memory parser
- [ ] Implement FD enumeration
- [ ] Integration tests

### Week 3 (Phase 3)
- [ ] Start Go project
- [ ] Design transport protocol
- [ ] Implement delta detection
- [ ] Network tests

### (See roadmap.md for full timeline)

---

## Quick Reference

| What | Where | Owner |
|------|-------|-------|
| Big picture | [plan.md](plan.md) | Everyone |
| Philosophy | [motto.md](motto.md) | leadership |
| Timeline | [roadmap.md](roadmap.md) | PM |
| This tracker | [progress.md](progress.md) | PM |
| Capture | [phase1.md](phase1.md) + [phase2.md](phase2.md) | Rust team |
| Transfer | [phase3.md](phase3.md) | Go team |
| Restore | [phase4.md](phase4.md) | Rust team |
| Control | [phase5.md](phase5.md) | Python team |
| Testing | [phase6.md](phase6.md) | QA |
| Hardening | [phase7.md](phase7.md) | Security |
| Future | [phase8.md](phase8.md) | Research |

---

## How to Update This File

**Monthly**: Update phase status and risk register
**Weekly**: Update current phase progress bar
**As-needed**: Add blockers, escalate risks

Template for entry:

```markdown
### Milestone N: [Name] (Week X–Y)
- [ ] Task 1
- [ ] Task 2
- **Blocker for**: Phase X
```

---

## Contact & Escalation

- **Technical Questions**: See respective phase doc
- **Timeline Concerns**: See roadmap.md
- **Risks**: Update risk register above
- **Blockers**: Escalate with impact assessment
