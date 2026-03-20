# Roadmap: Wraith Development Timeline

## Phase Overview

```
Phase 1: Rust Foundation         [2 weeks]
    └─ ptrace, register capture
    
Phase 2: Memory Snapshot         [3 weeks]
    └─ /proc parsing, serialization
    
Phase 3: Go Transport Layer      [3 weeks]
    └─ Network protocol, delta sync
    
Phase 4: Rust Restorer           [3 weeks]
    └─ Process resurrection, restore
    
Phase 5: Python Orchestration    [2 weeks]
    └─ Integration, pre-flight, rollback
    
Phase 6: Full Integration Test   [2 weeks]
    └─ End-to-end single workload
    
Phase 7: Hardening              [3 weeks]
    └─ Error cases, monitoring, safety
    
Phase 8: Beyond v1              [Future]
    └─ Threads, sockets, devices
```

**Total v1 time estimate**: ~18–20 weeks
**Parallel work**: Phases 1, 2, 3 can overlap after week 3

## Detailed Milestones

### Block 1: Fundamentals (Weeks 1–5)
- [ ] Week 1: Rust ptrace / Go project setup / Python scaffold
- [ ] Week 2: Register capture working for x86-64
- [ ] Week 3: Memory map parsing, region identification
- [ ] Week 4: Protobuf schema finalized
- [ ] Week 5: Memory serialization correctness validated

### Block 2: Transport (Weeks 6–10)
- [ ] Week 6: Go server/client basic HTTP
- [ ] Week 7: Delta detection algorithm
- [ ] Week 8: Streaming and checksum validation
- [ ] Week 9: Network resilience (retries, timeouts)
- [ ] Week 10: Transport layer load testing

### Block 3: Restore (Weeks 11–14)
- [ ] Week 11: Restorer trampoline design
- [ ] Week 12: Virtual address reconstruction
- [ ] Week 13: Register restore + detach
- [ ] Week 14: Single process restore correctness

### Block 4: Integration (Weeks 15–18)
- [ ] Week 15: Python orchestration core
- [ ] Week 16: Pre-flight verification script
- [ ] Week 17: Rollback and error handling
- [ ] Week 18: End-to-end test with real workload

### Block 5: Hardening (Weeks 19–21)
- [ ] Week 19: Edge cases, corrupted snapshots, network faults
- [ ] Week 20: Monitoring, observability, logging
- [ ] Week 21: Performance profiling, optimization

## Success Gates

| Gate | Criterion | Owner |
|------|-----------|-------|
| Gate 1 | Capture deterministic register state | Rust |
| Gate 2 | Memory snapshot is byte-identical on restore | Rust + Tests |
| Gate 3 | Network transfer reliable at 1GB scale | Go + Integration |
| Gate 4 | Single process migrates without reset | System |
| Gate 5 | Rollback works in all failure cases | System |

## Go/No-Go Decision Points

- **End of Phase 3**: Is transport layer stable? If not, redesign.
- **End of Phase 5**: Can we migrate a real job? If not, debug integration.
- **End of Phase 7**: Is this safe for non-experimental use? If not, iterate safety.

## Post-v1 Roadmap (2026–2027)

All post-v1 features are tracked in [phase8.md](phase8.md) as sub-phases.

| Sub-Phase | Focus | Difficulty | Est. Weeks |
|-----------|-------|-----------|------------|
| Phase 8.1 | Multi-threaded process support | High | 4–6 |
| Phase 8.2 | TCP socket state transfer | Very High | 6–8 |
| Phase 8.3 | Device fd + inotify + epoll | Very High | 8+ |
| Phase 8.4 | Cross-architecture (x86→ARM) | Research | 12+ |
| Phase 8.5 | Live migration (sub-second pause) | Very High | 6–8 |
| Phase 8.6 | Container-aware checkpointing | Medium | 3–4 |
| Phase 8.7 | Observability and debugging tools | Low | 2–3 |

**Recommended v1 → v2 path:**
- v1.1: Phase 8.1 (multi-threading)
- v1.2: Phase 8.2 (TCP sockets)
- v2.0: Phase 8.1 + 8.2 + 8.7

See [phase8.md](phase8.md) for implementation sketches and decision trees.
