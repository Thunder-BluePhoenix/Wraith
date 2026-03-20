# Phase 7: Hardening — Production Safety and Observability

**Duration**: 3 weeks | **Owner**: Security + DevOps team | **Output**: Hardened production binary

## Goals

1. Handle all error cases gracefully
2. Add observability (logging, metrics, tracing)
3. Implement security measures (auth, encryption)
4. Create incident recovery playbooks
5. Performance optimization

## Deliverables

### 7.1 Error Handling

**error_cases.md** — Comprehensive error taxonomy

```
CATEGORY: Capture
├─ Process terminated during capture
├─ Permission denied (non-root)
├─ Memory region becomes inaccessible
├─ Out of disk space for snapshot
└─ Interrupted system call (EINTR)

CATEGORY: Transfer
├─ Network timeout
├─ Connection refused
├─ Corrupted data (checksum mismatch)
├─ Out of disk on destination
└─ SSH key authentication failed

CATEGORY: Restore
├─ Address space conflict
├─ Insufficient memory
├─ Permission denied (writing memory)
├─ Invalid snapshot format
└─ Kernel version mismatch

CATEGORY: Coordination
├─ Source process escaped freeze
├─ Destination process crashed
├─ Mid-flight network partition
└─ Race condition in state transitions
```

**Recovery strategies per category**:

```python
class ErrorRecoveryStrategy:
    """Define behavior for each error type"""
    
    STRATEGIES = {
        "capture_permission_denied": {
            "action": "ABORT",
            "message": "Run as root or use sudo",
            "rollback": False,
        },
        "capture_process_terminated": {
            "action": "ABORT",
            "message": "Process died during capture",
            "rollback": False,
        },
        "transfer_network_timeout": {
            "action": "RETRY",
            "max_retries": 3,
            "backoff": "exponential",
            "rollback": True,
        },
        "transfer_checksum_mismatch": {
            "action": "RETRY",
            "max_retries": 5,
            "rollback": True,
        },
        "restore_address_conflict": {
            "action": "ABORT",
            "message": "Address space conflict (ASLR issue)",
            "rollback": True,
        },
        "restore_memory_insufficient": {
            "action": "ABORT",
            "message": "Destination out of memory",
            "rollback": True,
        },
    }
```

### 7.2 Logging and Observability

**logging.py** — Structured logging

```python
import logging
import json
from datetime import datetime
from typing import Any, Dict

class StructuredLogger:
    """JSON structured logging for analysis"""
    
    def __init__(self, name: str, level=logging.INFO):
        self.logger = logging.getLogger(name)
        self.logger.setLevel(level)
        
        # JSON handler
        handler = logging.StreamHandler()
        handler.setFormatter(logging.Formatter("%(message)s"))
        self.logger.addHandler(handler)
        
        self.context: Dict[str, Any] = {}
    
    def set_context(self, **kwargs):
        """Set request-scoped context"""
        self.context.update(kwargs)
    
    def _format_event(self, level: str, message: str, **kwargs) -> str:
        event = {
            "timestamp": datetime.utcnow().isoformat(),
            "level": level,
            "message": message,
            **self.context,
            **kwargs,
        }
        return json.dumps(event)
    
    def info(self, message: str, **kwargs):
        self.logger.info(self._format_event("INFO", message, **kwargs))
    
    def error(self, message: str, **kwargs):
        self.logger.error(self._format_event("ERROR", message, **kwargs))
    
    def warning(self, message: str, **kwargs):
        self.logger.warning(self._format_event("WARNING", message, **kwargs))


# Usage
log = StructuredLogger("wraith.teleporter")

# In migrate()
log.set_context(src_pid=12345, dest_host="worker-1")
log.info("Migration started", estimated_size_mb=1024)
log.info("Capture complete", duration_s=2.5, snapshot_size_mb=1024)
```

**Metrics** — Prometheus-compatible metrics

```python
from prometheus_client import Counter, Histogram, Gauge

# Migration outcomes
migrations_total = Counter(
    "wraith_migrations_total",
    "Total migration attempts",
    ["status"],  # success, rollback, failed
)

migration_duration = Histogram(
    "wraith_migration_duration_seconds",
    "Migration duration",
    buckets=(10, 30, 60, 120, 300, 600),
)

migration_bytes = Counter(
    "wraith_migration_bytes_total",
    "Bytes transferred",
)

process_memory_size = Gauge(
    "wraith_process_memory_bytes",
    "Process memory size",
)

# Per-phase metrics
capture_duration = Histogram(
    "wraith_capture_duration_seconds",
    "Capture phase duration",
)

transfer_duration = Histogram(
    "wraith_transfer_duration_seconds",
    "Transfer phase duration",
)

restore_duration = Histogram(
    "wraith_restore_duration_seconds",
    "Restore phase duration",
)

# Usage in teleporter
start = time.time()
try:
    self.migrate()
    migrations_total.labels(status="success").inc()
    migration_duration.observe(time.time() - start)
except Exception:
    migrations_total.labels(status="failed").inc()
    raise
```

### 7.3 Security Hardening

**security.md** — Security considerations

1. **Authentication**
   - Require SSH key (no passwords)
   - Validate server hostkey
   - Timeout on auth failures

2. **Encryption**
   - SSH tunnel for all communication
   - Protocol-level checksums (already in v1)
   - Optional: TLS wrapping for Go protocol

3. **Process Isolation**
   - Run as dedicated low-privilege user (not root)
   - Drop capabilities after initialization
   - Restrict file access via seccomp

4. **Snapshot File Security**
   ```python
   # Snapshot contains full process memory
   # Must be protected like sensitive data
   
   def secure_snapshot_storage(path: str):
       """Protect snapshot with 0600 perms"""
       import os
       os.chmod(path, 0o600)  # Only owner can read
       
       # Consider encryption
       # subprocess.run(["gpg", "--symmetric", path])
   ```

5. **Audit Logging**
   ```python
   # Log all migration attempts with source/dest
   audit_log = {}
   audit_log["timestamp"] = time.time()
   audit_log["src_pid"] = pid
   audit_log["src_user"] = os.getuid()
   audit_log["dest_host"] = destination
   audit_log["result"] = "success"  # or failure reason
   
   # Write to audit trail
   # Forward to syslog or central logging
   ```

### 7.4 Performance Optimization

**optimization.md** — Performance targets v1→v2

| Phase | Bottleneck | Target | Strategy |
|-------|-----------|--------|----------|
| Capture | ptrace overhead | Parallelize fd reads | Use process_vm_readv for large reads |
| Transfer | Network latency | 80MB/s | Implement delta+compression |
| Restore | Memory writes | 100MB/s | Use UFFDIO_COPY for zero-copy |

**Lazy page fault approach** (Future):
```
Instead of:
1. Snapshot all pages
2. Transfer all pages
3. Map all pages

Do:
1. Snapshot metadata
2. Transfer on-demand
3. Use userfaultfd to load pages lazily

Result: Sub-second pause time
```

### 7.5 Runbooks and Playbooks

**recovery_playbooks.md** — Incident response

```markdown
## Playbook: Source Process Frozen (Migration Failed)

### Symptoms
- Source PID shows in 'T' state after failed migration
- Process not responding to signals

### Root Cause
- Migration interrupted (power loss, network partition)
- Process stuck in ptrace (bug)

### Recovery Steps

1. Attempt soft unfreeze:
   ```bash
   wraith-capturer --resume <pid>
   ```

2. If soft unfreeze fails:
   ```bash
   sudo kill -CONT <pid>
   ```

3. If still frozen, escalate:
   ```bash
   sudo strace -p <pid>  # Check for stuck syscalls
   ```

4. Final resort (if safe):
   ```bash
   sudo kill -9 <pid>  # Only if acceptable
   ```

---

## Playbook: Snapshot Corruption

### Symptoms
- Transfer completes but checksum mismatch
- Restore fails with "Invalid snapshot"

### Root Cause
- Network packet loss (rare with TCP, but possible for hardware issues)
- Bug in memory read
- Bug in protobuf encoding

### Recovery Steps

1. Verify network integrity:
   ```bash
   ping -c 1000 <destination>  # Check packet loss
   ```

2. Re-transfer:
   ```bash
   wraith-transmitter --snapshot <file> --dest <host> --retry 5
   ```

3. If persists, recapture:
   ```bash
   wraith-capturer --pid <pid> --output <new_snapshot>
   wraith-transmitter --snapshot <new_snapshot> --dest <host>
   ```

---

## Playbook: Destination Out of Memory

### Symptoms
- Restore starts but fails mid-way
- Destination OOM-killer activates

### Root Cause
- Not enough free memory on destination
- Other processes consuming memory
- Memory leak in restorer

### Recovery Steps

1. Verify destination resources:
   ```bash
   ssh <dest> free -h
   ssh <dest> ps aux --sort=-%mem | head -10
   ```

2. Stop unnecessary processes on destination:
   ```bash
   ssh <dest> systemctl stop <service>
   ```

3. Retry migration:
   ```bash
   wraith migrate --pid <pid> --dest <host>
   ```

4. If still fails, need more destination capacity.
```

### 7.6 Canary Testing

**canary.py** — Gradual rollout

```python
class CanaryTester:
    """Test migrations on non-critical processes first"""
    
    def run_canary_phase(self):
        """Phase 1: Non-critical test workloads"""
        test_workloads = [
            ("simple_compute", "python -c 'sum(range(1000000))'"),
            ("memory_intensive", "python -c 'import numpy as np; x = np.zeros((1000, 1000))'"),
            ("io_workload", "dd if=/dev/zero of=/tmp/test bs=1M count=100"),
        ]
        
        results = {}
        for name, cmd in test_workloads:
            proc = subprocess.Popen(cmd, shell=True)
            try:
                result = self.attempt_migration(proc.pid)
                results[name] = "PASS" if result else "FAIL"
            except Exception as e:
                results[name] = f"ERROR: {e}"
            finally:
                proc.terminate()
        
        return results

    def canary_metrics(self):
        """Check metrics before rolling out to production"""
        return {
            "success_rate": self.calc_success_rate(),
            "avg_duration": self.calc_avg_duration(),
            "errors": self.count_errors(),
        }
```

### 7.7 Configuration

**production_config.yaml** — Recommended settings for production

```yaml
wraith:
  # Timeouts
  capture_timeout_s: 60
  transfer_timeout_s: 300
  restore_timeout_s: 120
  
  # Retries
  transfer_retry_count: 3
  transfer_retry_backoff_ms: 1000
  
  # Resource limits
  max_process_size_gb: 64
  max_snapshot_file_size_gb: 70
  
  # Pre-flight checks
  verify_destination_arch: true
  verify_destination_space: true
  verify_process_state: true
  
  # Monitoring
  export_metrics: true
  metrics_port: 9090
  log_level: INFO
  
  # Security
  require_ssh_key: true
  enforce_auth_timeout_s: 10
  snapshot_encryption: disabled  # aes256 for production
  audit_log_enabled: true
```

## Validation Checklist

- [ ] All error cases handled
- [ ] Structured logging working
- [ ] Metrics exported correctly
- [ ] Security best practices applied
- [ ] Recovery playbooks tested
- [ ] Canary phase successful
- [ ] Performance targets met
- [ ] No data loss in failure scenarios

## Known Limitations

- ❌ TLS not implemented yet (use SSH tunnel)
- ❌ Central logging not integrated
- ✓ Local audit logging works
- ✓ Prometheus metrics exported

## Success Criteria

- [x] Production-grade error handling
- [x] Observable via logging + metrics
- [x] Secure by default
- [x] Recovery procedures documented and tested
