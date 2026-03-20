# Wraith: Process Teleportation Engine

## Core Philosophy

**Move a running process. Not a container. Not a VM. Actual process.**

### What We're Solving
- Migrate long-running workloads without restart
- Cross-machine process mobility (same architecture, same OS)
- Transparent to the application — zero downtime
- Safety-first: rollback always possible until confirmed

### Design Principles

1. **Honesty over ambition**
   - v1 scope: single-threaded, no sockets, no GPU, x86-64 → x86-64, Linux
   - Leave socket/device/threading for v2+
   - Better to ship working than ship broken

2. **Modularity**
   - Rust: low-level kernel interaction
   - Go: network transport and streaming
   - Python: orchestration and safety logic
   - Each language does what it's best at

3. **Safety as architecture**
   - Source process stays frozen until destination confirms
   - No silent failures — explicit rollback
   - Checksums on every transferred byte
   - Pre-flight verification before any capture

4. **Learn from CRIU**
   - Study proven patterns but don't copy baggage
   - Cross-machine changes everything about the problem
   - Treat this as CRIU + transport, not CRIU clone

## Success Metric

Ship a working v1 that reliably moves a 6-hour computation job from laptop to server without losing state.
