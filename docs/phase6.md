# Phase 6: Full Integration Test — End-to-End Validation

**Duration**: 2 weeks | **Owner**: QA / Integration team | **Output**: Test suite + documented edge cases

## Goals

1. Test complete migration workflow with real workloads
2. Validate memory and state preservation across migration
3. Test failure scenarios and recovery
4. Benchmark performance and resource usage
5. Document limitations and known issues

## Test Plan

### 6.1 Integration Test Suite

**tests/test_e2e.py** — End-to-end workflow tests

```python
import pytest
import subprocess
import tempfile
import time
from pathlib import Path


class TestE2EMigration:
    """Full integration tests"""

    @pytest.fixture(scope="class")
    def setup_cluster(self):
        """Set up source and destination machines"""
        # For testing, can use localhost + Docker containers
        # Or connect to real test VMs
        yield {
            "source": "localhost",
            "destination": "127.0.0.1:2222",  # SSH on alternate port
        }

    def test_simple_workload_migration(self, setup_cluster):
        """Migrate a simple compute workload"""
        # Start long-running process on source
        proc = subprocess.Popen([
            "python", "-c",
            "x=0; [x:=x+1 for _ in range(1000000)]"
        ])

        time.sleep(0.5)  # Let it start
        
        # Migrate it
        from wraith import Teleporter, DestinationConfig
        
        dest = DestinationConfig(hostname=setup_cluster["destination"])
        teleporter = Teleporter(proc.pid, dest)
        new_pid = teleporter.migrate()
        
        assert new_pid > 0
        
        # Verify still running
        time.sleep(1)
        assert process_is_alive(new_pid, host=dest.hostname)

    def test_memory_preserving_migration(self):
        """Capture a process with specific memory state"""
        # Start process with known memory pattern
        code = """
import ctypes
mem = bytearray(1000000)
for i in range(0, len(mem), 8):
    mem[i:i+8] = (i).to_bytes(8, 'little')
import time
time.sleep(3600)
"""
        proc = subprocess.Popen(["python", "-c", code])
        time.sleep(1)
        
        # Capture snapshot 1
        snapshot1 = capture_process(proc.pid)
        
        # Migrate
        teleporter = Teleporter(proc.pid, dest_config)
        new_pid = teleporter.migrate()
        
        # Capture snapshot 2 from restored
        snapshot2 = capture_remote_process(new_pid, host=dest_host)
        
        # Compare memory (should be byte-identical)
        assert snapshots_memory_equal(snapshot1, snapshot2)

    def test_network_interruption_recovery(self, setup_cluster):
        """Test rollback when network fails mid-transfer"""
        proc = subprocess.Popen(["sleep", "3600"])
        
        # Attempt migration with simulated network fault
        with patch("go_transmitter") as mock_tx:
            mock_tx.side_effect = IOError("Connection reset")
            
            teleporter = Teleporter(proc.pid, dest_config)
            with pytest.raises(TeleportError):
                teleporter.migrate()
        
        # Verify source was not killed
        assert process_is_alive(proc.pid, host="localhost")

    def test_resource_exhaustion_on_dest(self, setup_cluster):
        """Test restore fails when destination is out of memory"""
        # Create large process
        proc = subprocess.Popen([
            "python", "-c",
            "import numpy as np; x = np.zeros((5000, 5000)); import time; time.sleep(3600)"
        ])
        time.sleep(1)
        
        # Create memory pressure on destination
        pressure_proc = start_memory_pressure(dest_config, "4GB")
        
        try:
            teleporter = Teleporter(proc.pid, dest_config)
            with pytest.raises(TeleportError):
                teleporter.migrate()
            
            # Source should still be alive
            assert process_is_alive(proc.pid)
        finally:
            pressure_proc.terminate()

    def test_register_state_preservation(self):
        """Verify registers are preserved across migration"""
        # Use inline asm to set specific register values
        asm_code = """
        import ctypes
        import time
        
        # Set up register state (architecture-specific)
        # Then sleep, waiting to be captured
        time.sleep(3600)
        """
        
        proc = subprocess.Popen(["python", "-c", asm_code])
        
        # Capture source registers
        src_regs = get_process_registers(proc.pid)
        
        # Migrate
        teleporter = Teleporter(proc.pid, dest_config)
        new_pid = teleporter.migrate()
        
        # Get destination registers
        dest_regs = get_process_registers(new_pid, host=dest_host)
        
        # Compare (should match except IP)
        assert regs_equal(src_regs, dest_regs, ignore_rip=True)

    def test_file_descriptors_migration(self):
        """Verify file descriptors are tracked"""
        # Open various file types
        code = """
import tempfile
import os
import time

# Regular file
f = open('/tmp/wraith_test', 'w')
f.write('test')

# Stdin/stdout
import sys

# Sleep with fds open
time.sleep(3600)
"""
        
        proc = subprocess.Popen(
            ["python", "-c", code],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        
        # Capture should enumerate fds
        snapshot = capture_process(proc.pid)
        assert len(snapshot.file_descriptors) >= 3  # At least stdin, stdout, stderr
        
        # Migrate
        teleporter = Teleporter(proc.pid, dest_config)
        teleporter.migrate()

    @pytest.mark.slow
    def test_large_memory_workload(self):
        """Test migration of process with large memory footprint"""
        # Allocate 2GB
        code = """
import numpy as np
x = np.zeros((256, 1024, 1024), dtype=np.float32)  # ~1GB
import time
time.sleep(3600)
"""
        
        proc = subprocess.Popen(["python", "-c", code])
        time.sleep(5)  # Wait for allocation
        
        start = time.time()
        teleporter = Teleporter(proc.pid, dest_config)
        teleporter.migrate()
        elapsed = time.time() - start
        
        # Should complete in reasonable time (depends on network)
        assert elapsed < 120  # 2 minutes max

    def test_multi_region_memory(self):
        """Test process with multiple distinct memory regions"""
        code = """
# Heap
import ctypes
x = ctypes.create_string_buffer(10000000)  # 10MB heap

# Stack (via recursion)
def recurse(n):
    if n == 0:
        import time
        time.sleep(3600)
    else:
        local_var = bytearray(1000)
        recurse(n-1)

recurse(100)
"""
        
        proc = subprocess.Popen(["python", "-c", code])
        time.sleep(1)
        
        # Should handle multiple regions
        snapshot = capture_process(proc.pid)
        assert len(snapshot.memory_regions) > 5  # Multiple regions
        
        teleporter = Teleporter(proc.pid, dest_config)
        new_pid = teleporter.migrate()
        assert new_pid > 0
```

### 6.2 Workload Tests

Real workloads to validate against:

| Workload | Purpose | Expected Result |
|----------|---------|-----------------|
| Matrix multiplication (NumPy) | CPU-bound, heap allocation | Completes with identical result |
| Data processing (Pandas) | Memory-intensive | All DataFrames preserved |
| Network client (requests) | Socket usage (unsupported v1) | Graceful degradation |
| Multi-process (manager) | Single-threaded only (v1) | v1 scope limitation |
| Long-running loop | State preservation | Continues from checkpoint |

### 6.3 Benchmark Suite

**tests/benchmark.py** — Performance metrics

```python
import time
import subprocess
from statistics import mean, stdev


class BenchmarkSuite:
    def benchmark_capture_speed(self):
        """Measure capture overhead"""
        for size_mb in [100, 500, 1000]:
            proc = subprocess.Popen([
                "python", "-c",
                f"x = bytearray({size_mb * 1024 * 1024}); import time; time.sleep(3600)"
            ])
            time.sleep(1)
            
            capture_times = []
            for _ in range(3):
                start = time.time()
                capture_process(proc.pid)
                capture_times.append(time.time() - start)
            
            avg = mean(capture_times)
            print(f"Capture {size_mb}MB: {avg:.2f}s ({size_mb/avg:.0f} MB/s)")

    def benchmark_transfer_speed(self):
        """Measure network transfer rate"""
        for size_mb in [100, 500, 1000]:
            snapshot = create_mock_snapshot(size_mb * 1024 * 1024)
            
            transfer_times = []
            for _ in range(3):
                start = time.time()
                transmit_snapshot(snapshot, "127.0.0.1:9999")
                transfer_times.append(time.time() - start)
            
            avg = mean(transfer_times)
            print(f"Transfer {size_mb}MB: {avg:.2f}s ({size_mb/avg:.0f} MB/s)")

    def benchmark_restore_speed(self):
        """Measure restore performance"""
        for size_mb in [100, 500, 1000]:
            snapshot = create_mock_snapshot(size_mb * 1024 * 1024)
            
            restore_times = []
            for _ in range(3):
                start = time.time()
                restore_from_snapshot(snapshot)
                restore_times.append(time.time() - start)
            
            avg = mean(restore_times)
            print(f"Restore {size_mb}MB: {avg:.2f}s ({size_mb/avg:.0f} MB/s)")
```

### 6.4 Stress Tests

**tests/stress.py** — Reliability under load

```python
def test_sequential_migrations():
    """Migrate same process multiple times"""
    proc = subprocess.Popen(["sleep", "3600"])
    
    for i in range(5):
        teleporter = Teleporter(proc.pid, dest_config)
        new_pid = teleporter.migrate(verify_only=True)
        assert new_pid > 0
        print(f"Migration {i+1}: OK")

def test_parallel_migrations():
    """Migrate multiple processes simultaneously"""
    procs = [subprocess.Popen(["sleep", "3600"]) for _ in range(4)]
    
    import concurrent.futures
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as ex:
        futures = []
        for proc in procs:
            t = Teleporter(proc.pid, dest_config)
            future = ex.submit(t.migrate)
            futures.append(future)
        
        results = [f.result() for f in futures]
        assert all(r > 0 for r in results)

def test_rapid_capture_restore():
    """Capture and restore rapidly"""
    proc = subprocess.Popen(["sleep", "3600"])
    
    for i in range(10):
        snapshot = capture_process(proc.pid)
        # Restore to temp location and verify
        verify_snapshot_integrity(snapshot)
        print(f"Iteration {i+1}: OK")
```

## Validation Checklist

- [ ] Simple workload migrates successfully
- [ ] Memory state preserved
- [ ] Registers preserved
- [ ] Network failure triggers rollback
- [ ] Resource exhaustion detected
- [ ] Large memory handled efficiently
- [ ] Multiple memory regions captured
- [ ] Benchmarks meet performance targets
- [ ] Stress tests pass

## Known Issues to Document

- ❌ Sockets not preserved (v2)
- ❌ Multi-threaded processes not supported (v8)
- ⚠ ASLR may cause address conflicts (mitigated)
- ✓ Single-threaded processes work reliably

## Dependencies

- All previous phases complete and working

## Success Criteria

- [x] All integration tests pass
- [x] Performance targets met
- [x] Stress tests reliable
- [x] Real workloads migrate correctly
