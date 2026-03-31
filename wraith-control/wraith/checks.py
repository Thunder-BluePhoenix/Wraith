"""
Pre-flight validation for Wraith migration.

Run ALL checks before freezing the source process. Failures here are cheap;
failures after freezing require rollback.

Check categories:
  LOCAL  — inspects the source machine; no SSH needed
  REMOTE — requires an active RemoteSession

All checks append a CheckResult to self.results. The caller sees the full
picture even when individual checks fail, which aids debugging.

Usage::

    checker = PreflightChecker(pid=1234, config=migration_cfg, session=ssh_sess)
    if not checker.run_all():
        for r in checker.blocking_failures():
            print(r.message)
        raise PreflightError(...)
"""

from __future__ import annotations

import logging
import os
import platform
import subprocess
from dataclasses import dataclass
from typing import List, Optional

from wraith.config import MigrationConfig
from wraith.remote import RemoteSession

log = logging.getLogger("wraith.checks")


@dataclass
class CheckResult:
    name:     str
    passed:   bool
    message:  str
    blocking: bool   # If True, a failure aborts migration


class PreflightChecker:
    """
    Runs all pre-flight checks and accumulates results.

    Call `run_all()` for the standard check suite. Individual check methods
    can be called separately for debugging.
    """

    def __init__(
        self,
        pid: int,
        config: MigrationConfig,
        session: Optional[RemoteSession] = None,
    ) -> None:
        self._pid = pid
        self._cfg = config
        self._sess = session
        self.results: List[CheckResult] = []

    # ── Public API ────────────────────────────────────────────────────────────

    def run_all(self) -> bool:
        """
        Run the full check suite.

        Returns True if all blocking checks pass.
        Non-blocking failures are recorded but do not affect the return value.
        """
        # Local checks — no SSH required.
        self.check_source_process_exists()
        self.check_source_process_permissions()
        self.check_source_process_size()
        self.check_source_arch()

        # Remote checks — SSH required.
        if self._sess is not None:
            self.check_destination_reachable()
            self.check_destination_arch()
            self.check_destination_resources()
            self.check_binaries_present()
        else:
            log.warning("No RemoteSession — skipping remote checks")
            self._record(CheckResult(
                name="remote_checks",
                passed=False,
                message="Remote checks skipped (no SSH session provided)",
                blocking=False,
            ))

        passed = all(r.passed for r in self.results if r.blocking)
        log.debug(
            "Pre-flight: %d checks, %d passed, %d failed (%d blocking failures)",
            len(self.results),
            sum(1 for r in self.results if r.passed),
            sum(1 for r in self.results if not r.passed),
            sum(1 for r in self.results if not r.passed and r.blocking),
        )
        return passed

    def blocking_failures(self) -> List[CheckResult]:
        return [r for r in self.results if not r.passed and r.blocking]

    def warnings(self) -> List[CheckResult]:
        return [r for r in self.results if not r.passed and not r.blocking]

    # ── Local checks ──────────────────────────────────────────────────────────

    def check_source_process_exists(self) -> CheckResult:
        """Verify /proc/<pid> exists."""
        exists = os.path.isdir(f"/proc/{self._pid}")
        result = CheckResult(
            name="source_process_exists",
            passed=exists,
            message=(
                f"Process {self._pid} found"
                if exists
                else f"No process with PID {self._pid} — check with `ps aux`"
            ),
            blocking=True,
        )
        return self._record(result)

    def check_source_process_permissions(self) -> CheckResult:
        """
        Verify we can ptrace the process.

        We check /proc/<pid>/mem readability as a proxy for ptrace permission,
        because attempting an actual ptrace attach would freeze the process.
        """
        mem_path = f"/proc/{self._pid}/mem"
        # Try to open mem — only works if we have ptrace access.
        try:
            fd = os.open(mem_path, os.O_RDONLY)
            os.close(fd)
            passed = True
            msg = f"ptrace access to PID {self._pid} confirmed"
        except PermissionError:
            passed = False
            msg = (
                f"Cannot read {mem_path} — insufficient permissions. "
                f"Run as root or set /proc/sys/kernel/yama/ptrace_scope=0"
            )
        except FileNotFoundError:
            # Process disappeared between checks.
            passed = False
            msg = f"Process {self._pid} disappeared mid-check"

        return self._record(CheckResult(
            name="source_process_permissions",
            passed=passed,
            message=msg,
            blocking=True,
        ))

    def check_source_process_size(self) -> CheckResult:
        """Reject processes larger than CaptureConfig.max_memory_gb."""
        limit_gb = self._cfg.capture.max_memory_gb
        try:
            rss_kb = _read_proc_status_kb(self._pid, "VmRSS")
            rss_gb = rss_kb / (1024 * 1024)
            ok = rss_gb <= limit_gb
            msg = (
                f"RSS {rss_gb:.2f} GB ≤ limit {limit_gb:.0f} GB"
                if ok
                else f"RSS {rss_gb:.2f} GB exceeds limit {limit_gb:.0f} GB"
            )
        except Exception as e:
            rss_gb = 0.0
            ok = False
            msg = f"Cannot read process memory usage: {e}"

        return self._record(CheckResult(
            name="source_process_size",
            passed=ok,
            message=msg,
            blocking=not ok,
        ))

    def check_source_arch(self) -> CheckResult:
        """Verify the source machine is x86-64 (Wraith v1 only supports x86-64)."""
        arch = platform.machine()
        ok = arch == "x86_64"
        return self._record(CheckResult(
            name="source_arch",
            passed=ok,
            message=f"Source arch: {arch}" + ("" if ok else " (must be x86_64)"),
            blocking=True,
        ))

    # ── Remote checks ─────────────────────────────────────────────────────────

    def check_destination_reachable(self) -> CheckResult:
        """Verify we can SSH to the destination and run a simple command."""
        assert self._sess is not None
        dest_addr = self._cfg.destination.ssh_addr
        try:
            out, _, _ = self._sess.exec("echo wraith-ping", timeout=5)
            ok = "wraith-ping" in out
            msg = f"Destination {dest_addr} reachable" if ok else f"Unexpected SSH response from {dest_addr}"
        except Exception as e:
            ok = False
            msg = f"Cannot reach {dest_addr}: {e}"

        return self._record(CheckResult(
            name="destination_reachable",
            passed=ok,
            message=msg,
            blocking=True,
        ))

    def check_destination_arch(self) -> CheckResult:
        """Destination arch must match source (same x86-64)."""
        assert self._sess is not None
        src_arch = platform.machine()
        try:
            dest_arch, _, _ = self._sess.exec("uname -m", timeout=5)
            dest_arch = dest_arch.strip()
            ok = src_arch == dest_arch
            msg = f"Arch: source={src_arch} dest={dest_arch}"
        except Exception as e:
            ok = False
            dest_arch = "unknown"
            msg = f"Cannot check destination arch: {e}"

        return self._record(CheckResult(
            name="architecture_match",
            passed=ok,
            message=msg,
            blocking=True,
        ))

    def check_destination_resources(self) -> CheckResult:
        """
        Destination must have enough free memory to host the process.

        We require 1.5× RSS to account for snapshot overhead + kernel bookkeeping.
        """
        assert self._sess is not None
        try:
            rss_kb    = _read_proc_status_kb(self._pid, "VmRSS")
            needed_kb = int(rss_kb * 1.5)

            # `free -k` → line: "Mem: total used free shared buff/cache available"
            out, _, _ = self._sess.exec("free -k", timeout=5)
            avail_kb  = _parse_free_available(out)

            ok  = avail_kb >= needed_kb
            msg = (
                f"Dest available: {avail_kb // 1024} MB, "
                f"needed: {needed_kb // 1024} MB (1.5× RSS)"
            )
        except Exception as e:
            ok  = False
            msg = f"Cannot determine resource availability: {e}"

        return self._record(CheckResult(
            name="destination_resources",
            passed=ok,
            message=msg,
            blocking=True,
        ))

    def check_binaries_present(self) -> CheckResult:
        """Verify wraith-restorer and wraith-receiver exist on the destination."""
        assert self._sess is not None
        dest = self._cfg.destination
        missing = []

        for binary_path in (dest.wraith_restorer_path, dest.wraith_receiver_path):
            try:
                _, _, code = self._sess.exec(
                    f"command -v {binary_path} || test -x {binary_path}",
                    check=False,
                )
                if code != 0:
                    missing.append(binary_path)
            except Exception:
                missing.append(binary_path)

        ok = len(missing) == 0
        msg = (
            "Wraith binaries found on destination"
            if ok
            else f"Missing on destination: {', '.join(missing)}"
        )
        return self._record(CheckResult(
            name="binaries_present",
            passed=ok,
            message=msg,
            blocking=True,
        ))

    # ── Internal ─────────────────────────────────────────────────────────────

    def _record(self, result: CheckResult) -> CheckResult:
        level = logging.DEBUG if result.passed else (
            logging.WARNING if not result.blocking else logging.ERROR
        )
        log.log(level, "Check %-30s %s  %s",
                result.name,
                "PASS" if result.passed else "FAIL",
                result.message)
        self.results.append(result)
        return result


# ── Helpers ───────────────────────────────────────────────────────────────────

def _read_proc_status_kb(pid: int, field: str) -> int:
    """Read a kB value from /proc/<pid>/status."""
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            if line.startswith(f"{field}:"):
                # Format: "VmRSS:\t1234 kB"
                return int(line.split()[1])
    raise KeyError(f"{field!r} not found in /proc/{pid}/status")


def _parse_free_available(free_output: str) -> int:
    """
    Parse `free -k` output and return available kB.

    Handles both old (available in column 7) and new (available = last col) formats.
    """
    for line in free_output.splitlines():
        if line.startswith("Mem:"):
            parts = line.split()
            # `free -k` columns: Mem: total used free shared buff/cache available
            if len(parts) >= 7:
                return int(parts[6])   # "available" column
            elif len(parts) >= 4:
                return int(parts[3])   # "free" column (older kernels)
    raise ValueError(f"Cannot parse `free -k` output: {free_output!r}")
