# Wraith: Process Teleportation Engine

Move a running process from one machine to another. Not a container. Not a VM. Actual process.

---

## What It Does

Wraith captures the complete state of a running Linux process — memory, registers, file descriptors — serializes it, transfers it over the network, and resurrects it on a destination machine. The source process stays frozen until the destination confirms success. If anything goes wrong, the source is unfrozen and nothing is lost.

**v1 scope (intentionally constrained):**
- Single-threaded processes only
- No open network sockets
- Same architecture (x86-64 → x86-64)
- File-backed storage only
- Linux kernel only

This covers real workloads: long-running compute jobs, data processing scripts, simulation runs.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│         Python Control Layer (wraith-control)   │
│    Orchestration, Pre-flight checks, Rollback   │
└──────────────────┬──────────────────────────────┘
                   │ subprocess / SSH
        ┌──────────┴───────────┐
        │                      │
┌───────▼──────────┐  ┌────────▼──────────┐
│  wraith-capturer │  │ wraith-transmitter │
│  wraith-restorer │  │  wraith-receiver   │
│  (Rust)          │  │  (Go)              │
│  ptrace layer    │  │  Delta transfer    │
│  Memory dump     │  │  Checksum verify   │
│  FD enumeration  │  │  Streaming write   │
└───────────┬──────┘  └────────┬───────────┘
            │                  │
            └────────┬─────────┘
                     │ Protobuf (wraith.proto)
             ┌───────▼────────┐
             │ ProcessSnapshot │
             └────────────────┘
```

### Components

| Component | Language | Role |
|-----------|----------|------|
| `wraith-rust/` | Rust | ptrace capture, memory dump, process restore |
| `wraith-go/` | Go | network transport, delta sync, streaming |
| `wraith-control/` | Python | orchestration, CLI, pre-flight, rollback |
| `wraith-proto/` | Protobuf | shared snapshot schema |

---

## Repository Layout

```
wraith/
├── README.md                  (this file)
├── docs/
│   ├── motto.md               Philosophy and principles
│   ├── plan.md                Architecture and build strategy
│   ├── roadmap.md             Timeline and milestones
│   ├── progress.md            Phase status dashboard
│   ├── phase1.md              Rust foundation (ptrace, registers)
│   ├── phase2.md              Memory snapshot (maps, serialization)
│   ├── phase3.md              Go transport (delta, streaming)
│   ├── phase4.md              Rust restorer (trampoline, resume)
│   ├── phase5.md              Python orchestration (CLI, rollback)
│   ├── phase6.md              Integration tests
│   ├── phase7.md              Hardening (security, observability)
│   └── phase8.md              Beyond v1 (threads, sockets, devices)
│
├── wraith-rust/               Rust: capture + restore binary
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── capturer.rs
│       ├── restorer.rs
│       ├── ptrace_ops.rs
│       ├── memory.rs
│       ├── registers.rs
│       ├── fd_enum.rs
│       ├── snapshot.rs
│       └── error.rs
│
├── wraith-go/                 Go: network transport binary
│   ├── go.mod
│   └── cmd/
│       ├── transmitter/
│       └── receiver/
│   └── pkg/
│       ├── transport/
│       ├── delta/
│       └── stream/
│
├── wraith-control/            Python: CLI + orchestration
│   ├── setup.py
│   └── wraith/
│       ├── cli.py
│       ├── teleporter.py
│       ├── checks.py
│       ├── remote.py
│       └── rollback.py
│
└── proto/
    └── wraith.proto           Shared Protobuf schema
```

---

## Quick Start

```bash
# Migrate PID 12345 to remote host
wraith migrate --pid 12345 --destination worker.example.com --key ~/.ssh/id_rsa

# Capture snapshot only (debug)
wraith capture --pid 12345

# Transfer snapshot manually (debug)
wraith transfer --snapshot /tmp/snap.pb --dest worker.example.com:9999
```

---

## Build Order

Phases must build in sequence — each depends on the previous:

```
Phase 1 (Rust: ptrace + registers)
  → Phase 2 (Rust: memory snapshot + Protobuf schema)
    → Phase 3 (Go: transport layer)        [can start after Phase 2]
    → Phase 4 (Rust: restorer trampoline)  [needs Phase 2 schema]
      → Phase 5 (Python: orchestration)   [needs Phases 2, 3, 4]
        → Phase 6 (Integration tests)
          → Phase 7 (Hardening)
```

See [docs/roadmap.md](docs/roadmap.md) for timeline and [docs/progress.md](docs/progress.md) for current status.

---

## Key Design Decisions

**Why freeze the source until confirmed?**
Source process stays in `ptrace` STOP state — not killed — until the destination sends explicit success. If anything fails, one `PTRACE_DETACH` call unfreezes it. No data loss is possible before the commit point.

**Why three languages?**
Rust owns the kernel interface (ptrace, mmap, memory writes) where safety is non-negotiable. Go owns the network layer where goroutines and streaming IO excel. Python owns orchestration where iteration speed and SSH libraries matter. Each does exactly what it's best at.

**Why not CRIU?**
CRIU is single-machine. Wraith adds the cross-machine transport layer, streaming restore, and the Python safety net (pre-flight + rollback). The ptrace patterns are CRIU-inspired but the transport and orchestration are new.

---

## Documentation Index

| Doc | Purpose |
|-----|---------|
| [motto.md](docs/motto.md) | Project philosophy |
| [plan.md](docs/plan.md) | Architecture + build strategy |
| [roadmap.md](docs/roadmap.md) | Timeline + milestones |
| [progress.md](docs/progress.md) | Current status per phase |
| [phase1–7.md](docs/) | Implementation details per phase |
| [phase8.md](docs/phase8.md) | Post-v1 research roadmap |
