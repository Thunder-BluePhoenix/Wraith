"""
Exception hierarchy for Wraith orchestration.

All exceptions carry a human-readable message and, where relevant,
a `hint` string that the CLI can display as a suggested next action.

Design rule: never raise bare Exception from wraith code. Always use
a type from this module so the CLI can distinguish fatal errors from
warnings and handle them appropriately.
"""

from __future__ import annotations

from typing import Optional


class WraithError(Exception):
    """Base class for all Wraith errors."""

    def __init__(self, message: str, hint: Optional[str] = None) -> None:
        super().__init__(message)
        self.hint = hint

    def __str__(self) -> str:
        base = super().__str__()
        if self.hint:
            return f"{base}\n  Hint: {self.hint}"
        return base


# ── Pre-flight errors ─────────────────────────────────────────────────────────

class PreflightError(WraithError):
    """
    A blocking pre-flight check failed.

    Migration will not start. The source process is untouched.
    """

    def __init__(self, check_name: str, reason: str, hint: Optional[str] = None) -> None:
        self.check_name = check_name
        super().__init__(
            f"Pre-flight check '{check_name}' failed: {reason}",
            hint=hint,
        )


class ProcessNotFoundError(PreflightError):
    def __init__(self, pid: int) -> None:
        super().__init__(
            check_name="source_process_exists",
            reason=f"No process with PID {pid}",
            hint="Verify the PID with `ps aux | grep <pid>`",
        )


class PermissionError_(PreflightError):
    """Rename to avoid shadowing builtins."""
    def __init__(self, pid: int) -> None:
        super().__init__(
            check_name="source_process_permissions",
            reason=f"Cannot ptrace PID {pid} — insufficient permissions",
            hint="Run wraith as root or set /proc/sys/kernel/yama/ptrace_scope to 0",
        )


class ArchitectureMismatchError(PreflightError):
    def __init__(self, src: str, dest: str) -> None:
        super().__init__(
            check_name="architecture_match",
            reason=f"Source arch {src!r} != destination arch {dest!r}",
            hint="Wraith v1 only supports same-arch migration (x86-64 → x86-64)",
        )


class DestinationUnreachableError(PreflightError):
    def __init__(self, addr: str, cause: str) -> None:
        super().__init__(
            check_name="destination_reachable",
            reason=f"Cannot SSH to {addr}: {cause}",
            hint="Check SSH keys, firewall rules, and that sshd is running on the destination",
        )


class InsufficientResourcesError(PreflightError):
    def __init__(self, required_gb: float, available_gb: float) -> None:
        super().__init__(
            check_name="destination_resources",
            reason=f"Need {required_gb:.1f} GB, destination has {available_gb:.1f} GB free",
            hint="Free memory on the destination or migrate a smaller process",
        )


class BinariesNotFoundError(PreflightError):
    def __init__(self, binary: str, hostname: str) -> None:
        super().__init__(
            check_name="binaries_present",
            reason=f"{binary!r} not found on {hostname}",
            hint=f"Install Wraith binaries on {hostname}: cargo install wraith-capturer",
        )


# ── Capture errors ────────────────────────────────────────────────────────────

class CaptureError(WraithError):
    """wraith-capturer failed. Source process was NOT frozen (capturer handles rollback)."""


class ProcessTooBigError(CaptureError):
    def __init__(self, rss_gb: float, limit_gb: float) -> None:
        super().__init__(
            f"Process RSS {rss_gb:.1f} GB exceeds limit {limit_gb:.1f} GB",
            hint="Increase CaptureConfig.max_memory_gb or migrate a smaller workload",
        )


# ── Transfer errors ───────────────────────────────────────────────────────────

class TransferError(WraithError):
    """
    Network transfer failed.

    The source process is frozen. Rollback will unfreeze it.
    """


class ReceiverStartError(TransferError):
    def __init__(self, hostname: str, cause: str) -> None:
        super().__init__(
            f"Could not start wraith-receiver on {hostname}: {cause}",
            hint="Check that wraith-receiver is installed and the transfer port is open",
        )


# ── Restore errors ────────────────────────────────────────────────────────────

class RestoreError(WraithError):
    """
    wraith-restorer failed on the destination.

    The restored process did not start. Rollback will unfreeze the source.
    """


# ── Rollback errors ───────────────────────────────────────────────────────────

class RollbackError(WraithError):
    """
    The rollback attempt itself failed.

    This is the worst-case scenario: migration failed AND the source process
    may be stuck in ptrace-stop state (shows as 'T' in `ps`).
    """

    def __init__(self, original_error: str, rollback_cause: str) -> None:
        self.original_error = original_error
        self.rollback_cause = rollback_cause
        super().__init__(
            f"Migration failed ({original_error}) AND rollback failed ({rollback_cause})",
            hint=(
                "Source process may be frozen (state 'T' in `ps aux`). "
                "Run `wraith-capturer resume --pid <pid>` on the source machine to unfreeze it."
            ),
        )
