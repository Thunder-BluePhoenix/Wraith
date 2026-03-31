"""
Unit tests for wraith.checks.PreflightChecker.

These tests mock out OS and SSH calls so they run on any machine
(including Windows / non-Linux CI) without needing ptrace or SSH.
"""

import os
import sys
import pytest

# Skip entire module on non-Linux (some checks use /proc).
# Individual tests that don't need /proc are still importable.
IS_LINUX = sys.platform == "linux"


from wraith.checks import PreflightChecker, _parse_free_available, _read_proc_status_kb
from wraith.config import MigrationConfig


def _make_config(hostname: str = "dest.example.com") -> MigrationConfig:
    return MigrationConfig.simple(hostname=hostname)


# ── _parse_free_available ─────────────────────────────────────────────────────

class TestParseFreeAvailable:
    def test_modern_format(self):
        output = (
            "              total        used        free      shared  buff/cache   available\n"
            "Mem:       16000000     4000000     8000000      500000     3500000     9000000\n"
            "Swap:       2000000           0     2000000\n"
        )
        assert _parse_free_available(output) == 9_000_000

    def test_old_format_fallback(self):
        # Older kernels omit the "available" column.
        output = "Mem:       16000000     4000000     8000000\n"
        assert _parse_free_available(output) == 8_000_000

    def test_raises_on_garbage(self):
        with pytest.raises(ValueError):
            _parse_free_available("this is not free output\n")


# ── check_source_process_exists ───────────────────────────────────────────────

class TestCheckSourceProcessExists:
    def test_nonexistent_pid(self):
        cfg = _make_config()
        checker = PreflightChecker(pid=999999999, config=cfg)
        result = checker.check_source_process_exists()
        assert not result.passed
        assert result.blocking

    @pytest.mark.skipif(not IS_LINUX, reason="Needs /proc")
    def test_current_process(self):
        cfg = _make_config()
        checker = PreflightChecker(pid=os.getpid(), config=cfg)
        result = checker.check_source_process_exists()
        assert result.passed


# ── check_source_arch ─────────────────────────────────────────────────────────

class TestCheckSourceArch:
    @pytest.mark.skipif(not IS_LINUX, reason="platform.machine() irrelevant on non-Linux")
    def test_x86_64_passes_on_x86_64(self, monkeypatch):
        import platform
        monkeypatch.setattr(platform, "machine", lambda: "x86_64")
        cfg = _make_config()
        checker = PreflightChecker(pid=os.getpid(), config=cfg)
        result = checker.check_source_arch()
        assert result.passed

    def test_non_x86_64_fails(self, monkeypatch):
        import platform
        monkeypatch.setattr(platform, "machine", lambda: "aarch64")
        cfg = _make_config()
        checker = PreflightChecker(pid=1, config=cfg)
        result = checker.check_source_arch()
        assert not result.passed
        assert result.blocking


# ── check_source_process_size ─────────────────────────────────────────────────

class TestCheckSourceProcessSize:
    @pytest.mark.skipif(not IS_LINUX, reason="Needs /proc")
    def test_current_process_below_64gb(self):
        cfg = _make_config()
        checker = PreflightChecker(pid=os.getpid(), config=cfg)
        result = checker.check_source_process_size()
        # The test runner itself is definitely < 64 GB.
        assert result.passed

    def test_nonexistent_pid_fails(self):
        cfg = _make_config()
        checker = PreflightChecker(pid=999999999, config=cfg)
        result = checker.check_source_process_size()
        assert not result.passed


# ── run_all with mocked remote ────────────────────────────────────────────────

class TestRunAllNoSession:
    @pytest.mark.skipif(not IS_LINUX, reason="Needs /proc")
    def test_run_all_without_session_skips_remote(self):
        """Without a RemoteSession, remote checks are skipped (non-blocking)."""
        cfg = _make_config()
        checker = PreflightChecker(pid=os.getpid(), config=cfg, session=None)
        # This will run local checks; they may pass or fail depending on environment.
        checker.run_all()
        remote_check = next(
            (r for r in checker.results if r.name == "remote_checks"), None
        )
        assert remote_check is not None
        assert not remote_check.blocking  # skipped non-blocking warning


class TestRunAllWithMockedSession:
    @pytest.mark.skipif(not IS_LINUX, reason="Needs /proc")
    def test_all_pass_with_happy_path(self, monkeypatch):
        import platform
        monkeypatch.setattr(platform, "machine", lambda: "x86_64")

        from unittest.mock import MagicMock

        session = MagicMock()
        session.exec.side_effect = lambda cmd, **kw: (
            ("wraith-ping\n", "", 0)      if "echo" in cmd
            else ("x86_64\n", "", 0)      if "uname" in cmd
            else ("Mem:  16000000  4000000  8000000  0  0  12000000\n", "", 0)  if "free" in cmd
            else ("", "", 0)              # binaries check
        )

        cfg = _make_config()
        checker = PreflightChecker(pid=os.getpid(), config=cfg, session=session)
        checker.run_all()

        blocking_fails = checker.blocking_failures()
        # Some checks may fail in unusual environments; just verify remote ones pass.
        remote_fails = [r for r in blocking_fails if r.name in (
            "destination_reachable", "architecture_match",
            "destination_resources", "binaries_present",
        )]
        assert remote_fails == [], f"Unexpected remote failures: {remote_fails}"
