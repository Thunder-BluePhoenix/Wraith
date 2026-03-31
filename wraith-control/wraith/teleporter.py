"""
Teleporter — the main orchestration state machine for Wraith migration.

## Migration flow

    INIT → PREFLIGHT → CAPTURING → TRANSFERRING → RESTORING → COMPLETE
                                                              ↓ on error at any step
                                                           ROLLING_BACK → ROLLED_BACK

The state machine enforces that:
  1. Pre-flight runs before anything touches the source process.
  2. The source process is unfrozen (rolled back) on ANY failure after capture.
  3. The source process is terminated only AFTER the destination confirms success.
  4. Rollback failure is surfaced as RollbackError with the original cause included.

## Point of no return

`_terminate_source()` is the commit point. Before it runs, rollback is always
possible. After it runs, the source process is dead and rollback is moot.

## Subprocess calls

The Teleporter shells out to the three Wraith binaries:
  - wraith-capturer (local)
  - wraith-transmitter (local, connects to remote receiver)
  - wraith-restorer (remote via SSH)

This design keeps the Python layer thin: it wires the binaries together and
handles errors, not the actual ptrace / mmap logic.
"""

from __future__ import annotations

import logging
import os
import subprocess
import tempfile
import time
import uuid
from enum import Enum
from typing import Optional

from wraith.checks import PreflightChecker
from wraith.config import MigrationConfig
from wraith.exceptions import (
    CaptureError,
    PreflightError,
    RestoreError,
    RollbackError,
    TransferError,
    ReceiverStartError,
    WraithError,
)
from wraith.remote import RemoteSession

log = logging.getLogger("wraith.teleporter")


class MigrationState(Enum):
    INIT         = "init"
    PREFLIGHT    = "preflight"
    CAPTURING    = "capturing"
    TRANSFERRING = "transferring"
    RESTORING    = "restoring"
    COMPLETE     = "complete"
    ROLLING_BACK = "rolling_back"
    ROLLED_BACK  = "rolled_back"
    FAILED       = "failed"     # rollback also failed — operator action required


class MigrationResult:
    """Returned by Teleporter.migrate() on success."""

    def __init__(
        self,
        migration_id: str,
        source_pid: int,
        destination_pid: int,
        destination_host: str,
        elapsed_s: float,
        snapshot_size_bytes: int,
    ) -> None:
        self.migration_id       = migration_id
        self.source_pid         = source_pid
        self.destination_pid    = destination_pid
        self.destination_host   = destination_host
        self.elapsed_s          = elapsed_s
        self.snapshot_size_bytes = snapshot_size_bytes

    def __str__(self) -> str:
        return (
            f"Migration {self.migration_id[:8]}: "
            f"PID {self.source_pid} → {self.destination_host}:{self.destination_pid} "
            f"({self.elapsed_s:.1f}s, {self.snapshot_size_bytes // 1024 // 1024} MB)"
        )


class Teleporter:
    """
    Orchestrates a complete process migration.

    Usage::

        cfg = MigrationConfig.simple(hostname="worker-2", ssh_key="~/.ssh/id_rsa")
        t = Teleporter(pid=12345, config=cfg)
        result = t.migrate()
        print(result)
    """

    def __init__(self, pid: int, config: MigrationConfig) -> None:
        self._pid           = pid
        self._cfg           = config
        self._state         = MigrationState.INIT
        self._migration_id  = str(uuid.uuid4())
        self._snapshot_path: Optional[str] = None
        self._remote_snapshot_path: Optional[str] = None
        self._destination_pid: Optional[int] = None
        self._start_time    = 0.0
        self._session: Optional[RemoteSession] = None

        log.debug("Teleporter created: migration_id=%s pid=%d dest=%s",
                  self._migration_id, pid, config.destination.ssh_addr)

    # ── Public API ────────────────────────────────────────────────────────────

    def migrate(self, *, verify_only: bool = False) -> MigrationResult:
        """
        Perform a full migration.

        Args:
            verify_only: If True, restore the process on the destination but
                         do NOT terminate the source. Useful for smoke-testing
                         the restore path without committing.

        Returns:
            MigrationResult on success.

        Raises:
            PreflightError:  Check failed; source is untouched.
            CaptureError:    Capture failed; source is untouched (capturer rolls back).
            TransferError:   Transfer failed; source is frozen — rollback will unfreeze.
            RestoreError:    Restore failed; source is frozen — rollback will unfreeze.
            RollbackError:   Migration failed AND rollback failed — operator action needed.
        """
        self._start_time = time.monotonic()

        with RemoteSession(self._cfg.destination) as session:
            self._session = session
            try:
                session.connect()
                self._run_preflight(session)
                self._run_capture()
                self._run_transfer(session)
                self._run_restore(session)

                if not verify_only:
                    self._terminate_source()

                self._transition(MigrationState.COMPLETE)
                return self._build_result()

            except PreflightError:
                # Pre-flight: source untouched, just re-raise.
                self._transition(MigrationState.ROLLED_BACK)
                raise

            except (CaptureError, TransferError, RestoreError, WraithError) as exc:
                # After capture: source may be frozen, must unfreeze.
                self._rollback(original_error=str(exc))
                raise

            finally:
                self._cleanup_snapshot()
                self._session = None

    # ── Migration phases ──────────────────────────────────────────────────────

    def _run_preflight(self, session: RemoteSession) -> None:
        self._transition(MigrationState.PREFLIGHT)
        log.info("Running pre-flight checks...")

        checker = PreflightChecker(
            pid=self._pid,
            config=self._cfg,
            session=session,
        )
        passed = checker.run_all()

        if checker.warnings():
            for w in checker.warnings():
                log.warning("  [warn] %s: %s", w.name, w.message)

        if not passed:
            failures = checker.blocking_failures()
            first = failures[0]
            raise PreflightError(
                check_name=first.name,
                reason=first.message,
                hint=f"Run `wraith check --pid {self._pid} --destination ...` for full report",
            )

        log.info("Pre-flight: all %d checks passed", len(checker.results))

    def _run_capture(self) -> None:
        self._transition(MigrationState.CAPTURING)

        # Write snapshot to a temp file that we own.
        fd, path = tempfile.mkstemp(
            prefix=f"wraith_{self._migration_id[:8]}_",
            suffix=".pb",
        )
        os.close(fd)
        self._snapshot_path = path

        capturer = self._cfg.destination.wraith_capturer_path
        cmd = [
            capturer,
            "capture",
            "--pid",    str(self._pid),
            "--output", path,
        ]

        log.info("Capturing PID %d → %s ...", self._pid, path)
        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self._cfg.capture.freeze_timeout_s,
            )
        except subprocess.TimeoutExpired:
            raise CaptureError(
                f"wraith-capturer timed out after {self._cfg.capture.freeze_timeout_s}s",
                hint="Increase CaptureConfig.freeze_timeout_s or check ptrace permissions",
            )
        except FileNotFoundError:
            raise CaptureError(
                f"wraith-capturer not found: {capturer!r}",
                hint="Install Wraith binaries or check PATH",
            )

        if result.returncode != 0:
            raise CaptureError(
                f"wraith-capturer exited {result.returncode}: {result.stderr.strip()}",
                hint="See stderr above for details",
            )

        size = os.path.getsize(path)
        log.info("Captured: %s (%d MB)", path, size // 1024 // 1024)

    def _run_transfer(self, session: RemoteSession) -> None:
        self._transition(MigrationState.TRANSFERRING)
        assert self._snapshot_path is not None

        dest = self._cfg.destination
        remote_path = f"{dest.remote_snapshot_dir}/wraith_{self._migration_id[:8]}.pb"
        self._remote_snapshot_path = remote_path

        # Start the receiver on the destination first.
        log.info("Starting wraith-receiver on %s:%d ...", dest.hostname, dest.transfer_port)
        receiver_cmd = (
            f"{dest.wraith_receiver_path} "
            f"--listen 0.0.0.0:{dest.transfer_port} "
            f"--output {remote_path}"
        )
        try:
            session.exec_background(receiver_cmd)
        except Exception as e:
            raise ReceiverStartError(dest.hostname, str(e)) from e

        # Give the receiver a moment to bind.
        time.sleep(1.5)

        # Run the transmitter locally.
        transmitter = dest.wraith_transmitter_path
        cmd = [
            transmitter,
            "--snapshot", self._snapshot_path,
            "--dest",     dest.transfer_addr,
            "--retries",  str(self._cfg.transfer.retry_count),
            "--timeout",  str(self._cfg.transfer.timeout_s),
            "--pid",      str(self._pid),
        ]

        log.info("Transmitting to %s ...", dest.transfer_addr)
        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self._cfg.transfer.timeout_s + 30,
            )
        except subprocess.TimeoutExpired:
            raise TransferError(
                f"wraith-transmitter timed out after {self._cfg.transfer.timeout_s}s",
                hint="Check network bandwidth and increase TransferConfig.timeout_s",
            )
        except FileNotFoundError:
            raise TransferError(
                f"wraith-transmitter not found: {transmitter!r}",
                hint="Install Wraith Go binaries or check PATH",
            )

        if result.returncode != 0:
            raise TransferError(
                f"wraith-transmitter exited {result.returncode}: {result.stderr.strip()}"
            )

        log.info("Transfer complete")

    def _run_restore(self, session: RemoteSession) -> None:
        self._transition(MigrationState.RESTORING)
        assert self._remote_snapshot_path is not None

        dest    = self._cfg.destination
        restore = self._cfg.restore

        flags = f"--snapshot {self._remote_snapshot_path}"
        if restore.strict_fds:
            flags += " --strict-fds"

        cmd = f"{dest.wraith_restorer_path} {flags}"
        log.info("Restoring on %s ...", dest.hostname)

        try:
            out, err, code = session.exec(
                cmd,
                timeout=restore.startup_timeout_s,
                check=False,
            )
        except Exception as e:
            raise RestoreError(
                f"wraith-restorer invocation failed: {e}",
                hint="Check SSH connection and that wraith-restorer is installed on destination",
            ) from e

        if code == 2 and restore.strict_fds:
            raise RestoreError(
                "wraith-restorer: FD restoration failed (--strict-fds mode)",
                hint="Remove --strict-fds to allow migration with FD warnings",
            )
        elif code != 0:
            raise RestoreError(
                f"wraith-restorer exited {code}: {err.strip()}",
                hint=f"Check {dest.remote_snapshot_dir}/wraith_receiver.log on destination",
            )

        # Parse the restored PID from restorer output: "Process <pid> is now running"
        self._destination_pid = _parse_restored_pid(out)
        log.info("Restored PID %d on %s", self._destination_pid, dest.hostname)

        # Clean up remote snapshot unless configured to keep it.
        if not restore.keep_snapshot:
            try:
                session.exec(f"rm -f {self._remote_snapshot_path}")
            except Exception:
                log.debug("Could not remove remote snapshot %s", self._remote_snapshot_path)

    def _terminate_source(self) -> None:
        """
        Kill the source process. POINT OF NO RETURN.

        After this returns, rollback is impossible: the source is dead.
        We log a prominent message so operators can track the commit point.
        """
        log.info(
            "COMMIT: terminating source PID %d — destination PID %d is running on %s",
            self._pid, self._destination_pid or -1,
            self._cfg.destination.hostname,
        )
        try:
            import signal
            os.kill(self._pid, signal.SIGTERM)
            # Give the process a moment to exit cleanly.
            time.sleep(0.5)
            try:
                os.kill(self._pid, signal.SIGKILL)
            except ProcessLookupError:
                pass  # Already dead from SIGTERM
        except ProcessLookupError:
            log.debug("Source PID %d already gone", self._pid)

    # ── Rollback ──────────────────────────────────────────────────────────────

    def _rollback(self, original_error: str) -> None:
        """
        Unfreeze the source process after a failed migration.

        If the source was captured (frozen by ptrace), `wraith-capturer resume`
        detaches ptrace and lets the process continue running.
        If the source was never frozen, this is a no-op.
        """
        self._transition(MigrationState.ROLLING_BACK)
        log.warning("Rolling back: %s", original_error)

        capturer = self._cfg.destination.wraith_capturer_path
        try:
            result = subprocess.run(
                [capturer, "resume", "--pid", str(self._pid)],
                capture_output=True,
                text=True,
                timeout=10,
            )
            if result.returncode == 0:
                log.info("Rollback: source PID %d resumed", self._pid)
                self._transition(MigrationState.ROLLED_BACK)
            else:
                # Rollback also failed — escalate.
                self._transition(MigrationState.FAILED)
                raise RollbackError(
                    original_error=original_error,
                    rollback_cause=result.stderr.strip(),
                )
        except subprocess.TimeoutExpired:
            self._transition(MigrationState.FAILED)
            raise RollbackError(
                original_error=original_error,
                rollback_cause="wraith-capturer resume timed out",
            )
        except RollbackError:
            raise
        except Exception as e:
            self._transition(MigrationState.FAILED)
            raise RollbackError(
                original_error=original_error,
                rollback_cause=str(e),
            )

    # ── Cleanup ───────────────────────────────────────────────────────────────

    def _cleanup_snapshot(self) -> None:
        """Remove the local snapshot temp file."""
        if self._snapshot_path and os.path.exists(self._snapshot_path):
            try:
                os.unlink(self._snapshot_path)
                log.debug("Removed local snapshot %s", self._snapshot_path)
            except OSError:
                log.debug("Could not remove local snapshot %s", self._snapshot_path)

    # ── State machine ─────────────────────────────────────────────────────────

    def _transition(self, new_state: MigrationState) -> None:
        log.debug(
            "State: %s → %s  (migration %s)",
            self._state.value, new_state.value, self._migration_id[:8]
        )
        self._state = new_state

    # ── Result builder ────────────────────────────────────────────────────────

    def _build_result(self) -> MigrationResult:
        assert self._destination_pid is not None
        elapsed = time.monotonic() - self._start_time
        size = (
            os.path.getsize(self._snapshot_path)
            if self._snapshot_path and os.path.exists(self._snapshot_path)
            else 0
        )
        return MigrationResult(
            migration_id=self._migration_id,
            source_pid=self._pid,
            destination_pid=self._destination_pid,
            destination_host=self._cfg.destination.hostname,
            elapsed_s=elapsed,
            snapshot_size_bytes=size,
        )

    @property
    def state(self) -> MigrationState:
        return self._state

    @property
    def migration_id(self) -> str:
        return self._migration_id


# ── Helpers ───────────────────────────────────────────────────────────────────

def _parse_restored_pid(output: str) -> int:
    """
    Extract the restored PID from wraith-restorer output.

    Expected line: "Process <pid> is now running on this machine."
    Falls back to -1 if not found (non-fatal — migration still succeeded).
    """
    for line in output.splitlines():
        parts = line.strip().split()
        # "Process 12345 is now running..."
        if len(parts) >= 2 and parts[0] == "Process":
            try:
                return int(parts[1])
            except ValueError:
                continue
    log.warning("Could not parse restored PID from restorer output:\n%s", output)
    return -1
