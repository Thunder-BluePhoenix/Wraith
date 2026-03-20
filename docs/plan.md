# Project Plan: Wraith Process Teleportation Engine

## Scope

**v1 Target (Minimum Viable Teleport)**
- Single-threaded processes only
- No open network sockets
- No GPU or accelerators
- Same architecture only (x86-64 → x86-64)
- File-backed storage only
- Linux kernel only

This scope covers real workloads: compute jobs, data processing, simulations.

## Architecture Overview

```
┌─────────────────────────────────────────────────┐
│         Python Control Layer                    │
│    (Orchestration, Pre-flight, Rollback)        │
└──────┬──────────────────────────────┬───────────┘
       │                              │
       │                              │
┌──────▼──────────┐         ┌─────────▼──────────┐
│  Rust Capturer  │         │   Go Transmitter   │
│  ptrace layer   │         │ Delta + Streaming  │
│  Memory dump    │         │ Checksum validate  │
│  FD enumeration │         │ Network protocol   │
└─────────────────┘         └─────────────────────┘
       │                              │
       │          (Protobuf)          │
       └──────────────┬───────────────┘
                      │
              ┌───────▼────────┐
              │ ProcessSnapshot │
              │   (Binary)      │
              └─────────────────┘
```

## Key Technical Challenges

1. **Memory Capture** — Walk /proc/pid/maps, read all regions, serialize bytes
2. **Register Preservation** — ptrace(PTRACE_GETREGS/SETREGS) with arch validation
3. **File Descriptor Mapping** — Enumerate, categorize, reopen on destination
4. **Virtual Address Reconstruction** — mmap exact regions before writing
5. **Network Transport** — Delta transfer, streaming, checksums
6. **Orchestration Safety** — Freeze source, verify destination, rollback on failure

## Stack Rationale

| Layer | Language | Why |
|-------|----------|-----|
| Capture | Rust | Memory safety for ptrace, zero-cost abstractions |
| Transfer | Go | Concurrency, streaming, proven for large data |
| Control | Python | Fast iteration, C bindings, orchestration logic |

## Build Strategy

1. Prototype each phase independently
2. Integrate via well-defined protocols (Protobuf, HTTP/gRPC)
3. Test each layer in isolation before full integration
4. Hardest part last: rollback + failure scenarios

## Success Criteria per Phase

- **Phase 1**: Capture process state without corruption
- **Phase 2**: Serialize and retrieve memory correctly
- **Phase 3**: Transfer snapshot across network reliably
- **Phase 4**: Restore process to identical state
- **Phase 5**: End-to-end migration works for simple workload
- **Phase 6**: Full integration test suite passes with real workloads
- **Phase 7**: Production-grade safety, monitoring, and security
- **Phase 8**: Post-v1 research (threads, sockets, devices) — do not start until v1.0 ships

See `roadmap.md` for timeline and milestones.
