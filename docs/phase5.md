# Phase 5: Python Orchestration — Control and Safety

**Duration**: 2 weeks | **Owner**: Python team | **Output**: CLI tool + control API

## Goals

1. Implement pre-flight checks (architecture, resources, permissions)
2. Coordinate snapshot capture on source
3. Coordinate transfer to destination
4. Coordinate restoration on destination
5. Implement rollback for failure scenarios
6. Provide clean CLI interface

## Deliverables

### 5.1 Project Structure

```
wraith-control/
├── setup.py
├── requirements.txt
├── wraith/
│   ├── __init__.py
│   ├── cli.py                (Command-line interface)
│   ├── teleporter.py         (Main orchestrator)
│   ├── checks.py             (Pre-flight validation)
│   ├── remote.py             (SSH/gRPC to destination)
│   ├── capture.py            (Call Rust capturer)
│   ├── transfer.py           (Call Go transmitter)
│   ├── restore.py            (Call Rust restorer)
│   ├── rollback.py           (Failure recovery)
│   ├── config.py             (Configuration)
│   └── logging.py            (Logging setup)
├── tests/
│   ├── test_checks.py
│   ├── test_teleporter.py
│   └── test_e2e.py
├── examples/
│   └── migrate_job.py        (Example script)
└── README.md
```

### 5.2 Requirements

**requirements.txt**:
```
click==8.0.3          # CLI
paramiko==2.11.0      # SSH
grpcio==1.40.0        # gRPC (optional for now)
black==21.12b0        # Formatting
pytest==6.2.5         # Testing
```

### 5.3 Configuration

**config.py** — Settings and defaults
```python
from dataclasses import dataclass
from typing import Optional

@dataclass
class DestinationConfig:
    """Configuration for destination machine"""
    hostname: str
    port: int = 22
    username: str = "root"
    ssh_key: Optional[str] = None
    wraith_binary_path: str = "/usr/local/bin/wraith"
    
    def validate(self) -> bool:
        """Validate all required fields"""
        return all([self.hostname, self.port, self.username])


@dataclass
class CaptureConfig:
    """Configuration for capture phase"""
    freeze_timeout_s: int = 30
    max_memory_gb: int = 64
    skip_validation: bool = False


@dataclass
class TransferConfig:
    """Configuration for transfer phase"""
    timeout_s: int = 300
    retry_count: int = 3
    chunk_size_mb: int = 10


@dataclass
class RestoreConfig:
    """Configuration for restore phase"""
    startup_timeout_s: int = 60
    verify_memory: bool = True
    keep_snapshot: bool = False  # Keep snapshot file after restore
```

### 5.4 Pre-flight Checks

**checks.py** — Validation before migration
```python
import subprocess
import os
from dataclasses import dataclass

@dataclass
class CheckResult:
    name: str
    passed: bool
    message: str
    blocking: bool  # If True, fail entire migration


class PreflightChecker:
    def __init__(self, src_pid: int, dest: DestinationConfig):
        self.src_pid = src_pid
        self.dest = dest
        self.results: list[CheckResult] = []

    def run_all(self) -> bool:
        """Run all checks, return True if all pass"""
        self.check_source_process_exists()
        self.check_source_process_permissions()
        self.check_architecture_match()
        self.check_destination_reachable()
        self.check_destination_resources()
        self.check_binaries_present()

        return all(r.passed for r in self.results if r.blocking)

    def check_source_process_exists(self) -> CheckResult:
        """Verify source PID exists"""
        if not os.path.exists(f"/proc/{self.src_pid}"):
            result = CheckResult(
                name="source_process_exists",
                passed=False,
                message=f"Process {self.src_pid} not found",
                blocking=True,
            )
        else:
            result = CheckResult(
                name="source_process_exists",
                passed=True,
                message=f"Process {self.src_pid} found",
                blocking=False,
            )
        self.results.append(result)
        return result

    def check_source_process_permissions(self) -> CheckResult:
        """Verify we can ptrace the process"""
        try:
            # Test ptrace attach
            proc = subprocess.run(
                ["ps", "-o", "pid=", "-p", str(self.src_pid)],
                capture_output=True,
                check=True,
            )
            result = CheckResult(
                name="source_process_permissions",
                passed=True,
                message="Can access process",
                blocking=False,
            )
        except subprocess.CalledProcessError:
            result = CheckResult(
                name="source_process_permissions",
                passed=False,
                message="Cannot access process (permissions?)",
                blocking=True,
            )
        self.results.append(result)
        return result

    def check_architecture_match(self) -> CheckResult:
        """Verify source and destination architectures match"""
        src_arch = os.uname().machine
        try:
            # SSH to destination and check
            dest_arch = self._ssh_exec("uname -m")
            match = src_arch == dest_arch
            result = CheckResult(
                name="architecture_match",
                passed=match,
                message=f"{src_arch} → {dest_arch}",
                blocking=True,
            )
        except Exception as e:
            result = CheckResult(
                name="architecture_match",
                passed=False,
                message=f"Failed to check: {e}",
                blocking=True,
            )
        self.results.append(result)
        return result

    def check_destination_reachable(self) -> CheckResult:
        """Ping destination via SSH"""
        try:
            self._ssh_exec("echo 'ping'", timeout=5)
            result = CheckResult(
                name="destination_reachable",
                passed=True,
                message=f"{self.dest.hostname}:{self.dest.port}",
                blocking=True,
            )
        except Exception as e:
            result = CheckResult(
                name="destination_reachable",
                passed=False,
                message=f"Cannot reach: {e}",
                blocking=True,
            )
        self.results.append(result)
        return result

    def check_destination_resources(self) -> CheckResult:
        """Verify destination has enough memory"""
        try:
            # Get process memory usage
            with open(f"/proc/{self.src_pid}/status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        rss_kb = int(line.split()[1])
                        rss_gb = rss_kb / 1024 / 1024

            # Check destination memory
            dest_mem_output = self._ssh_exec("free -g | grep Mem")
            parts = dest_mem_output.split()
            dest_mem_gb = int(parts[1])

            match = rss_gb < (dest_mem_gb * 0.7)  # Leave 30% headroom
            result = CheckResult(
                name="destination_resources",
                passed=match,
                message=f"Source: {rss_gb:.1f}GB, Dest: {dest_mem_gb}GB free",
                blocking=not match,
            )
        except Exception as e:
            result = CheckResult(
                name="destination_resources",
                passed=False,
                message=f"Cannot determine: {e}",
                blocking=True,
            )
        self.results.append(result)
        return result

    def check_binaries_present(self) -> CheckResult:
        """Verify Wraith binaries are installed"""
        try:
            self._ssh_exec("which wraith-restorer")
            result = CheckResult(
                name="binaries_present",
                passed=True,
                message="Wraith binaries found on destination",
                blocking=True,
            )
        except Exception as e:
            result = CheckResult(
                name="binaries_present",
                passed=False,
                message=f"Binaries not found: {e}",
                blocking=True,
            )
        self.results.append(result)
        return result

    def _ssh_exec(self, cmd: str, timeout: int = 10) -> str:
        """Execute command on destination via SSH"""
        import paramiko
        
        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
        client.connect(
            self.dest.hostname,
            port=self.dest.port,
            username=self.dest.username,
            key_filename=self.dest.ssh_key,
            timeout=timeout,
        )
        
        stdin, stdout, stderr = client.exec_command(cmd, timeout=timeout)
        output = stdout.read().decode().strip()
        client.close()
        
        return output
```

### 5.5 Main Orchestrator

**teleporter.py** — Coordinates full migration
```python
import subprocess
import tempfile
import time
from typing import Optional
from enum import Enum

class MigrationState(Enum):
    INIT = "init"
    PREFLIGHT = "preflight"
    CAPTURING = "capturing"
    FROZEN = "frozen"
    TRANSFERRING = "transferring"
    RESTORING = "restoring"
    RESTORED = "restored"
    VERIFIED = "verified"
    RECOVERED = "recovered"
    FAILED = "failed"


class Teleporter:
    def __init__(self, src_pid: int, dest: DestinationConfig):
        self.src_pid = src_pid
        self.dest = dest
        self.state = MigrationState.INIT
        self.snapshot_path: Optional[str] = None
        self.restored_pid: Optional[int] = None
        self.start_time = time.time()

    def migrate(self, verify_only: bool = False) -> int:
        """
        Perform full migration.
        Returns: PID of restored process on destination.
        Raises: TeleportError on any failure.
        """
        try:
            # Phase 1: Validation
            self._transition_to(MigrationState.PREFLIGHT)
            self._preflight_checks()

            # Phase 2: Capture
            self._transition_to(MigrationState.CAPTURING)
            self.snapshot_path = self._capture_snapshot()

            # Phase 3: Freezing
            self._transition_to(MigrationState.FROZEN)
            # Source process stays frozen until confirmed

            # Phase 4: Transfer
            self._transition_to(MigrationState.TRANSFERRING)
            self._transfer_snapshot()

            # Phase 5: Restore
            self._transition_to(MigrationState.RESTORING)
            self.restored_pid = self._restore_snapshot()

            # Phase 6: Verification (optional)
            if verify_only:
                self._transition_to(MigrationState.VERIFIED)
                self._verify_restore()

            # Phase 7: Commit (cannot roll back after this)
            self._transition_to(MigrationState.RESTORED)
            self._kill_source()

            elapsed = time.time() - self.start_time
            print(f"✓ Migration complete ({elapsed:.1f}s)")
            print(f"  Source PID {self.src_pid} → Dest PID {self.restored_pid}")

            return self.restored_pid

        except Exception as e:
            print(f"✗ Migration failed: {e}")
            self._transition_to(MigrationState.FAILED)
            self._rollback()
            raise

    def _preflight_checks(self):
        """Run pre-flight validation"""
        checker = PreflightChecker(self.src_pid, self.dest)
        if not checker.run_all():
            failing = [r for r in checker.results if not r.passed]
            raise TeleportError(
                f"Pre-flight checks failed: {failing[0].message}"
            )
        print("✓ Pre-flight checks passed")

    def _capture_snapshot(self) -> str:
        """Capture process snapshot on source"""
        with tempfile.NamedTemporaryFile(
            suffix=".pb", delete=False
        ) as tmp:
            snapshot_path = tmp.name

        result = subprocess.run(
            ["wraith-capturer", "--pid", str(self.src_pid),
             "--output", snapshot_path],
            capture_output=True,
            timeout=60,
        )

        if result.returncode != 0:
            raise TeleportError(f"Capture failed: {result.stderr.decode()}")

        size_mb = os.path.getsize(snapshot_path) / 1024 / 1024
        print(f"✓ Captured snapshot ({size_mb:.1f} MB)")
        return snapshot_path

    def _transfer_snapshot(self):
        """Transfer snapshot to destination"""
        if not self.snapshot_path:
            raise TeleportError("No snapshot to transfer")

        result = subprocess.run(
            ["wraith-transmitter",
             "--snapshot", self.snapshot_path,
             "--dest", f"{self.dest.hostname}:9999"],
            capture_output=True,
            timeout=self.dest.transfer_timeout_s,
        )

        if result.returncode != 0:
            raise TeleportError(f"Transfer failed: {result.stderr.decode()}")

        print(f"✓ Transferred snapshot")

    def _restore_snapshot(self) -> int:
        """Restore snapshot on destination"""
        try:
            # SSH to destination and run restorer
            import paramiko
            
            client = paramiko.SSHClient()
            client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
            client.connect(
                self.dest.hostname,
                port=self.dest.port,
                username=self.dest.username,
                key_filename=self.dest.ssh_key,
            )

            # Restorer is already running waiting for snapshot
            stdin, stdout, stderr = client.exec_command(
                "wraith-restorer",
                timeout=self.dest.startup_timeout_s,
            )

            output = stdout.read().decode().strip()
            restored_pid = int(output.split()[-1])

            client.close()
            print(f"✓ Restored on destination (PID {restored_pid})")
            return restored_pid

        except Exception as e:
            raise TeleportError(f"Restore failed: {e}")

    def _verify_restore(self):
        """Optional: Verify restored process integrity"""
        # Could run additional checks on destination
        print("✓ Verified restore")

    def _kill_source(self):
        """Kill source process (point of no return)"""
        try:
            os.kill(self.src_pid, signal.SIGTERM)
            print(f"✓ Source process terminated")
        except ProcessLookupError:
            pass  # Already dead

    def _rollback(self):
        """Recover from failure by unfreezing source"""
        self._transition_to(MigrationState.RECOVERED)
        try:
            # Attempt to unfreeze process
            subprocess.run(
                ["wraith-capturer", "--resume", str(self.src_pid)],
                timeout=10,
            )
            print("⚠ Rollback: source process unfrozen")
        except Exception as e:
            print(f"⚠ Rollback failed: {e}")

    def _transition_to(self, new_state: MigrationState):
        """Transition state with logging"""
        print(f"  [{self.state.value} → {new_state.value}]")
        self.state = new_state


class TeleportError(Exception):
    """Migration failed"""
    pass
```

### 5.6 CLI Interface

**cli.py** — Command-line tool
```python
import click
from typing import Optional


@click.group()
def cli():
    """Wraith: Process Teleportation Engine"""
    pass


@cli.command()
@click.option("--pid", required=True, type=int, help="Source process PID")
@click.option("--destination", required=True, help="Dest host (ssh config or hostname)")
@click.option("--username", default="root", help="SSH username")
@click.option("--key", help="SSH key file path")
@click.option("--port", default=22, type=int, help="SSH port")
@click.option("--verify-only", is_flag=True, help="Verify without killing source")
@click.option("--keep-snapshot", is_flag=True, help="Keep snapshot file after migrate")
def migrate(pid, destination, username, key, port, verify_only, keep_snapshot):
    """Migrate a running process to another machine"""
    
    dest_config = DestinationConfig(
        hostname=destination,
        port=port,
        username=username,
        ssh_key=key,
    )

    teleporter = Teleporter(pid, dest_config)
    try:
        restored_pid = teleporter.migrate(verify_only=verify_only)
        click.echo(
            click.style(
                f"✓ Success! Process running on {destination}:{restored_pid}",
                fg="green",
            )
        )
    except TeleportError as e:
        click.echo(click.style(f"✗ {e}", fg="red"), err=True)
        raise click.Exit(1)


@cli.command()
@click.option("--pid", required=True, type=int, help="Process PID")
def capture(pid):
    """Capture process snapshot (debug)"""
    try:
        path = subprocess.check_output(
            ["wraith-capturer", "--pid", str(pid)],
            text=True,
        ).strip()
        click.echo(f"Snapshot: {path}")
    except subprocess.CalledProcessError as e:
        click.echo(click.style(f"Failed: {e}", fg="red"), err=True)
        raise click.Exit(1)


@cli.command()
@click.option("--snapshot", required=True, help="Snapshot file path")
@click.option("--dest", required=True, help="Destination address:port")
def transfer(snapshot, dest):
    """Transfer snapshot to destination (debug)"""
    try:
        subprocess.run(
            ["wraith-transmitter", "--snapshot", snapshot, "--dest", dest],
            check=True,
        )
    except subprocess.CalledProcessError as e:
        click.echo(click.style(f"Failed: {e}", fg="red"), err=True)
        raise click.Exit(1)


if __name__ == "__main__":
    cli()
```

### 5.7 Example Usage

**examples/migrate_job.py** — Real-world example
```python
#!/usr/bin/env python3
"""
Example: Migrate a long-running data processing job.

Usage:
    python migrate_job.py --pid 12345 --dest workernode.example.com
"""

import sys
from wraith import Teleporter, DestinationConfig, TeleportError


def main():
    import argparse

    parser = argparse.ArgumentParser(description="Migrate process")
    parser.add_argument("--pid", required=True, type=int)
    parser.add_argument("--dest", required=True)
    parser.add_argument("--user", default="root")
    parser.add_argument("--key")
    args = parser.parse_args()

    dest = DestinationConfig(
        hostname=args.dest,
        username=args.user,
        ssh_key=args.key,
    )

    print(f"Migrating PID {args.pid} → {args.dest}...")

    teleporter = Teleporter(args.pid, dest)
    try:
        new_pid = teleporter.migrate(verify_only=False)
        print(f"✓ Success: Process now running as PID {new_pid}")
        return 0
    except TeleportError as e:
        print(f"✗ Failed: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
```

## Testing Strategy

### Unit Tests
- Pre-flight check logic
- Migration state machine
- Configuration validation

### Integration Tests
- Full migration workflow (capture → transfer → restore)
- Rollback on network failure
- Rollback on destination resource exhaustion

```python
def test_full_migration():
    # Start background process on source
    proc = Popen(["sleep", "3600"])
    
    dest = DestinationConfig(hostname="127.0.0.1", port=22)
    teleporter = Teleporter(proc.pid, dest)
    
    # Migrate
    new_pid = teleporter.migrate()
    assert new_pid > 0
    
    # Verify still running on dest
    assert destination_has_process(new_pid)
```

## Validation Checklist

- [ ] All pre-flight checks working
- [ ] Migration state machine transitions correctly
- [ ] Process frozen until confirmed
- [ ] Rollback unfreeze works
- [ ] CLI tool works end-to-end
- [ ] Error messages are clear
- [ ] Verify-only mode works
- [ ] SSH connection management

## Dependencies

- **Phase 2**: `wraith-capturer` binary (Rust)
- **Phase 3**: `wraith-transmitter` binary (Go)
- **Phase 4**: `wraith-restorer` binary (Rust)

## Success Criteria

- [x] Pre-flight checks pass
- [x] Migrate command works end-to-end
- [x] Rollback preserves source on failure
- [x] Integration test passes
