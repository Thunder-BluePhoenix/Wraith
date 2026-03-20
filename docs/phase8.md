# Phase 8: Beyond v1 — Advanced Features Roadmap

**Duration**: Research & Planning | **Owner**: Architecture team | **Output**: RFC documents per feature

## Overview

Phase 8 encompasses research-grade features that require significant kernel-level work. These are not blockers for v1 but enable production use cases with sockets, threads, and other kernel handles.

## Phase 8.1: Multi-Threaded Process Support

**Difficulty**: High | **Estimated**: 4–6 weeks

### Goals
- Capture all threads in a process
- Preserve thread state (registers, stack, TLS)
- Restore thread group atomically
- Maintain thread-local storage

### Challenges

1. **Thread Enumeration**
   - Already available: /proc/pid/task/
   - Each thread is a separate LWP (lightweight process)

2. **Thread State Capture**
   - Each thread has its own:
     - Registers (GP + FP + vector)
     - Stack
     - TLS (thread-local storage)
   - Captured per-thread, not per-process

3. **Thread Synchronization**
   - All threads must be frozen simultaneously
   - Restore must resume all at once
   - Race condition: what if thread spawns during capture?

### Implementation Sketch

```rust
pub struct ThreadSnapshot {
    tid: u32,
    registers: Registers,
    stack_addr: u64,
    stack_size: usize,
    tls_addr: u64,  // fs base (x86-64)
}

pub fn capture_all_threads(pid: i32) -> Result<Vec<ThreadSnapshot>> {
    // 1. Enumerate threads
    let task_dir = format!("/proc/{}/task", pid);
    let mut threads = Vec::new();
    
    // 2. Freeze all threads
    for entry in fs::read_dir(task_dir)? {
        let tid_str = entry?.file_name().to_string_lossy().to_string();
        let tid = tid_str.parse::<i32>()?;
        
        ptrace::attach(tid)?;
        waitpid(tid, None)?;
    }
    
    // 3. Capture each thread
    for tid in tids {
        let thread_snap = capture_thread(tid)?;
        threads.push(thread_snap);
    }
    
    Ok(threads)
}
```

### Testing
- Multi-threaded workload (thread pool)
- Threads with different states (blocked on futex, running, etc.)
- Thread creation during capture (should fail gracefully)

---

## Phase 8.2: TCP Socket State Transfer

**Difficulty**: Very High | **Estimated**: 6–8 weeks

### Goals
- Preserve TCP connection state
- Transfer socket to new machine
- Re-establish connection with correct sequence numbers
- Transparent to application

### Challenges

1. **TCP Socket Anatomy**
   - 4-tuple: (src_ip, src_port, dst_ip, dst_port)
   - Sequence numbers (send + recv windows)
   - Socket options (SO_KEEPALIVE, TCP_NODELAY, etc.)
   - Data in flight (unread recv buffer, unsent send buffer)

2. **Cross-Machine Transfer Problem**
   - Source IP changes on destination
   - Remote endpoint (peer) doesn't know IP changed
   - Peer still sends packets to old IP
   - TCP connection will timeout and reset

3. **Possible Solutions**

   **Option A: IP Spoofing (LAN only)**
   ```
   - Destination assumes source IP via ARP
   - Peer continues sending to old IP
   - Destination intercepts and reconstructs TCP state
   - Requires Layer 2 control (not viable WAN)
   ```

   **Option B: Proxy Layer (Recommended)**
   ```
   Source Machine          Network               Destination Machine
   ┌──────────────────┐                        ┌──────────────────┐
   │ Process          │ TCP_REPAIR            │ Process          │
   │ Socket: :9999 ←──── 1.2.3.4:5000        │ Socket: :9999 ←──┐
   └────────┬─────────┘                        └──────────────────┘
            │                                           ▲
            │ (Proxy)                                  │ (Proxy)
            └───────────────────────────┬──────────────┘
                                        │
                                    1.2.3.4:5000
                                  (External peer)
   
   - Keep original IP alive on source via proxy
   - Proxy forwards packets to new location
   - Enables gradual cutover
   ```

   **Option C: Application-Level Resume**
   ```
   - Accept that sockets don't transfer
   - Provide library for app to save/restore connection state
   - Example: checkpoint database cursor position
   - Less transparent but works for any app
   ```

### Implementation Sketch

```c
// Freeze socket state using TCP_REPAIR (Linux 3.5+)

#include <netinet/tcp.h>

int freeze_socket(int sock) {
    int one = 1;
    // Enable TCP_REPAIR mode
    setsockopt(sock, IPPROTO_TCP, TCP_REPAIR, &one, sizeof(one));
    
    // Extract TCP state
    struct tcp_repair_opt opts;
    socklen_t len = sizeof(opts);
    getsockopt(sock, IPPROTO_TCP, TCP_REPAIR_OPTIONS, &opts, &len);
    
    // Can now read:
    // - snd_una (send unack'd)
    // - snd_nxt (send next)
    // - rcv_nxt (recv next)
    // - rcv_tstamp, etc.
    
    return 0;
}

int restore_socket(int sock, struct tcp_repair_opt *opts) {
    int one = 1;
    setsockopt(sock, IPPROTO_TCP, TCP_REPAIR, &one, sizeof(one));
    
    // Restore TCP state
    setsockopt(sock, IPPROTO_TCP, TCP_REPAIR_OPTIONS, opts, sizeof(*opts));
    
    // Disable repair mode to start receiving
    one = 0;
    setsockopt(sock, IPPROTO_TCP, TCP_REPAIR, &one, sizeof(one));
    
    return 0;
}
```

### Testing
- Simple echo server connection
- High-throughput connection
- Connection with unsent data
- Connection with unread data
- Multiple simultaneous sockets

---

## Phase 8.3: Device FDs and Kernel Handles

**Difficulty**: Very High | **Estimated**: 8+ weeks

### Goals
- Preserve device handles (GPU, /dev/null, etc.)
- Preserve inotify watches
- Preserve epoll instances
- Preserve timers (timerfd, eventfd)

### Challenges

1. **Kernel Object Serialization**
   - Device files cannot be "copied"
   - inotify watches are kernel state
   - epoll instances are internal kernel tables
   - These must be recreated from scratch

2. **Inotify Reconstruction**
   ```c
   // Original state:
   int wd = inotify_add_watch(fd, "/home/user", IN_ALL_EVENTS);
   
   // After migration:
   int new_fd = inotify_init();
   int new_wd = inotify_add_watch(new_fd, "/home/user", IN_ALL_EVENTS);
   // Note: new_wd may != old_wd
   
   // Application expects original wd, so mapping needed
   wd_map[old_wd] = new_wd;
   ```

3. **epoll Reconstruction**
   ```c
   // Original:
   int epfd = epoll_create(1);
   epoll_ctl(epfd, EPOLL_CTL_ADD, fd1, &ev1);
   epoll_ctl(epfd, EPOLL_CTL_ADD, fd2, &ev2);
   
   // After migration: Need to remember all registrations
   // And rebuild them on destination
   // But fd1, fd2 may have different FD numbers!
   ```

### Implementation Strategy

1. Enumerate all kernel handles via /proc/pid/fd
2. For each handle, determine type
3. For each type, determine recreation strategy
4. Rebuild in correct order (dependencies matter)

```python
def reconstruct_kernel_handles(pid, snapshot):
    """Rebuild kernel state from snapshot"""
    for handle in snapshot.kernel_handles:
        if handle.type == "inotify":
            recreate_inotify(handle)
        elif handle.type == "epoll":
            recreate_epoll(handle)
        elif handle.type == "timerfd":
            recreate_timerfd(handle)
        elif handle.type == "device":
            recreate_device(handle)
```

### Testing
- Process with inotify watches
- Process using epoll
- Process with GPIO/device access
- Complex interaction (epoll on inotify)

---

## Phase 8.4: Cross-Architecture Migration

**Difficulty**: Research | **Estimated**: 12+ weeks

### Goals
- Migrate x86-64 process to ARM64
- Handle instruction set differences
- Translate register state
- Map memory appropriately

### Challenges

1. **Instruction Incompatibility**
   - x86 RIP points to x86 code
   - ARM64 PC points to ARM code
   - Process code must be recompiled for target arch
   - Not possible for arbitrary processes

2. **Register Mapping**
   - x86-64: 16 GP registers, specific FPU layout
   - ARM64: 31 GP registers, different FPU (NEON)
   - Some registers have no direct equivalent

3. **Memory Layout**
   - x86-64: 47-bit address space (typical)
   - ARM64: 48–56-bit address space (configurable)
   - Virtual address conflict less likely but still possible

### Feasibility

Cross-architecture migration is fundamentally limited:
- **Interpreted languages** (Python, JS): Maybe, if VM is recompiled
- **JIT processes** (JVM, V8): Difficult, JIT must be re-triggered
- **Native binaries**: Not possible without recompilation

**Recommendation**: Not worth implementing for v1. Consider language VMs instead.

---

## Phase 8.5: Live Migration (Sub-second Pause)

**Difficulty**: Very High | **Estimated**: 6–8 weeks

### Goals
- Migrate with <100ms pause
- Application experiences no timeout
- Network connections survive

### Approach: Pre-copy + Dirty-tracking

```
Round 1 (background):
  - Capture process (freeze ~1s)
  - Send all memory (1-10s on network)
  - Resume source
  - Track dirty pages

Round 2 (background):
  - Send only dirty pages (fast, 100-500ms)

Round 3 (final):
  - Final brief freeze
  - Send last dirty pages
  - Update registers
  - Resume on destination
  - Total pause: 10-100ms
```

### Implementation

```rust
pub struct LiveMigration {
    prev_snapshot: ProcessSnapshot,
    current_snapshot: ProcessSnapshot,
    dirty_pages: Vec<usize>,
}

impl LiveMigration {
    pub fn pre_copy_round(&mut self, pid: i32) -> Result<()> {
        // While running, detect dirty pages
        // Track via /proc/pid/pagemap and /proc/kpageflags
        
        let prev_hashes = hash_all_pages(&self.prev_snapshot);
        let current_hashes = hash_all_pages(&self.current_snapshot);
        
        self.dirty_pages = detect_dirty_pages(&prev_hashes, &current_hashes);
        
        Ok(())
    }
}
```

### Challenges
- Kernel page tracking (not all kernels support needed features)
- Keeping snapshots in sync while process runs
- Handling concurrent modifications during round

---

## Phase 8.6: Container-Aware Checkpointing

**Difficulty**: Medium | **Estimated**: 3–4 weeks

### Goals
- Checkpoint containerized processes
- Preserve container context
- Restore with same container environment

### Approach

1. Capture container metadata:
   - Docker/Podman config
   - Environment variables
   - Mounted volumes
   - Network namespace

2. Use cgroup v2 interface for resource tracking

3. Restore in container-aware way:
   - Recreate container with same config
   - Restore process within container

### Unlikely in v1, but useful for future.

---

## Phase 8.7: Observability and Debugging

**Difficulty**: Low | **Estimated**: 2–3 weeks

### Goals
- Provide debug insights into migration
- Trace ptrace syscalls
- Monitor resource usage

### Tools

```bash
# Trace ptrace syscalls
strace -e trace=ptrace wraith migrate --pid 12345 --dest host

# Monitor memory during migration
watch -n 0.1 'cat /proc/12345/status | grep VmRSS'

# Network traffic
tcpdump -i any tcp port 9999

# Metrics query
curl localhost:9090/metrics | grep wraith_
```

---

## Decision Tree: Which Phase 8 Features Should You Implement?

```
Does your use case require...

├─ Sockets?
│  └─ YES → Implement Phase 8.2 (Medium effort, high value)
│
├─ Multi-threaded apps?
│  └─ YES → Implement Phase 8.1 (High effort, essential for many apps)
│
├─ Device access (GPU, /dev/*)?
│  └─ YES → Implement Phase 8.3 (Will take forever)
│
├─ Cross-architecture?
│  └─ YES → Use container + v1 instead (not worth it)
│
└─ Sub-100ms pause times?
   └─ YES → Implement Phase 8.5 (Research-grade, >10 weeks)
```

## Recommended Path to v2

1. **v1.0** (Current roadmap): Single-threaded, no sockets, same-arch
2. **v1.1** (2–4 weeks): Multi-threading (Phase 8.1)
3. **v1.2** (4–6 weeks): TCP sockets (Phase 8.2)
4. **v2.0** (6+ weeks): All of above + observability (Phase 8.7)

## Known Hard Problems

| Problem | Reason | Difficulty |
|---------|--------|-----------|
| Cross-arch | Binary incompatibility | Unsolvable for native |
| Device handles | Kernel-managed resources | Research-grade |
| Live migration | Concurrent state tracking | Very hard |
| Multi-tenant | Permissions + isolation | Out of scope |

## Success Criteria (Not Yet)

- Do not implement Phase 8 until v1.0 ships and validates in production.
- Revisit based on real-world constraints.
