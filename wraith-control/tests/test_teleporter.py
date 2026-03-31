"""
Unit tests for wraith.teleporter.Teleporter.

All subprocess calls and SSH sessions are mocked so these tests run
on any platform and do not require real Wraith binaries.
"""

from __future__ import annotations

import os
import sys
import pytest
from unittest.mock import MagicMock, patch, call

IS_LINUX = sys.platform == "linux"

from wraith.config import MigrationConfig
from wraith.exceptions import (
    CaptureError,
    PreflightError,
    RestoreError,
    RollbackError,
    TransferError,
)
from wraith.teleporter import MigrationState, Teleporter, _parse_restored_pid


# ── _parse_restored_pid ───────────────────────────────────────────────────────

class TestParseRestoredPid:
    def test_standard_output(self):
        output = (
            "Restore complete:\n"
            "  restored pid  : 42\n"
            "  regions mapped: 12\n"
            "\nProcess 42 is now running on this machine.\n"
        )
        assert _parse_restored_pid(output) == 42

    def test_returns_minus_one_if_not_found(self):
        assert _parse_restored_pid("no pid here") == -1

    def test_handles_empty_string(self):
        assert _parse_restored_pid("") == -1


# ── MigrationState transitions ────────────────────────────────────────────────

class TestMigrationState:
    def test_initial_state_is_init(self):
        cfg = MigrationConfig.simple(hostname="h")
        t = Teleporter(pid=1, config=cfg)
        assert t.state == MigrationState.INIT

    def test_migration_id_is_uuid_like(self):
        cfg = MigrationConfig.simple(hostname="h")
        t = Teleporter(pid=1, config=cfg)
        assert len(t.migration_id) == 36  # UUID format


# ── Successful migration (all mocked) ────────────────────────────────────────

class TestSuccessfulMigration:
    def _make_teleporter(self) -> Teleporter:
        cfg = MigrationConfig.simple(hostname="dest.example.com")
        return Teleporter(pid=os.getpid() if IS_LINUX else 1, config=cfg)

    @pytest.mark.skipif(not IS_LINUX, reason="PreflightChecker reads /proc")
    def test_successful_flow(self, tmp_path, monkeypatch):
        import platform
        monkeypatch.setattr(platform, "machine", lambda: "x86_64")

        snapshot_file = tmp_path / "snap.pb"
        snapshot_file.write_bytes(b"\x00" * 1024)

        # Mock RemoteSession
        mock_session = MagicMock()
        mock_session.__enter__ = lambda s: s
        mock_session.__exit__ = MagicMock(return_value=False)
        mock_session.exec.side_effect = lambda cmd, **kw: (
            ("wraith-ping\n", "", 0)     if "echo" in cmd
            else ("x86_64\n", "", 0)     if "uname" in cmd
            else ("Mem: 16000000 4000000 8000000 0 0 12000000\n", "", 0) if "free" in cmd
            else ("", "", 0)             if "command -v" in cmd
            else (
                f"Process 9999 is now running on this machine.\n", "", 0
            )                            # wraith-restorer
        )

        # Mock subprocess.run for capturer + transmitter
        def mock_run(cmd, **kw):
            m = MagicMock()
            m.returncode = 0
            m.stdout = ""
            m.stderr = ""
            return m

        t = self._make_teleporter()

        with patch("wraith.teleporter.RemoteSession", return_value=mock_session), \
             patch("wraith.teleporter.subprocess.run", side_effect=mock_run), \
             patch("wraith.teleporter.tempfile.mkstemp",
                   return_value=(0, str(snapshot_file))), \
             patch("os.close"), \
             patch("os.path.getsize", return_value=1024), \
             patch("os.unlink"):

            result = t.migrate(verify_only=True)  # verify_only skips _terminate_source

        assert result.destination_pid == 9999
        assert t.state == MigrationState.COMPLETE


# ── Preflight failure ─────────────────────────────────────────────────────────

class TestPreflightFailure:
    def test_preflight_failure_leaves_state_rolled_back(self, monkeypatch):
        mock_checker = MagicMock()
        mock_checker.run_all.return_value = False
        mock_checker.warnings.return_value = []
        mock_checker.blocking_failures.return_value = [
            MagicMock(name="source_process_exists", message="No such process", blocking=True)
        ]

        mock_session = MagicMock()
        mock_session.__enter__ = lambda s: s
        mock_session.__exit__ = MagicMock(return_value=False)

        cfg = MigrationConfig.simple(hostname="h")
        t = Teleporter(pid=999999, config=cfg)

        with patch("wraith.teleporter.RemoteSession", return_value=mock_session), \
             patch("wraith.teleporter.PreflightChecker", return_value=mock_checker), \
             patch("wraith.teleporter.tempfile.mkstemp", return_value=(0, "/tmp/x.pb")), \
             patch("os.close"), patch("os.path.exists", return_value=False), \
             patch("os.unlink"):

            with pytest.raises(PreflightError):
                t.migrate()

        assert t.state == MigrationState.ROLLED_BACK


# ── Capture failure with rollback ─────────────────────────────────────────────

class TestCaptureFailureRollback:
    def test_capture_failure_triggers_rollback(self, monkeypatch):
        import subprocess

        mock_checker = MagicMock()
        mock_checker.run_all.return_value = True
        mock_checker.warnings.return_value = []
        mock_checker.blocking_failures.return_value = []

        mock_session = MagicMock()
        mock_session.__enter__ = lambda s: s
        mock_session.__exit__ = MagicMock(return_value=False)

        # capturer fails; resume succeeds
        def mock_run(cmd, **kw):
            m = MagicMock()
            if "capture" in cmd:
                m.returncode = 1
                m.stderr = "ptrace permission denied"
            else:  # resume
                m.returncode = 0
            return m

        cfg = MigrationConfig.simple(hostname="h")
        t = Teleporter(pid=1, config=cfg)

        with patch("wraith.teleporter.RemoteSession", return_value=mock_session), \
             patch("wraith.teleporter.PreflightChecker", return_value=mock_checker), \
             patch("wraith.teleporter.subprocess.run", side_effect=mock_run), \
             patch("wraith.teleporter.tempfile.mkstemp", return_value=(0, "/tmp/x.pb")), \
             patch("os.close"), patch("os.path.exists", return_value=False), \
             patch("os.unlink"):

            with pytest.raises(CaptureError):
                t.migrate()

        assert t.state == MigrationState.ROLLED_BACK


# ── Rollback failure (worst case) ─────────────────────────────────────────────

class TestRollbackFailure:
    def test_rollback_failure_raises_rollback_error(self):
        import subprocess

        mock_checker = MagicMock()
        mock_checker.run_all.return_value = True
        mock_checker.warnings.return_value = []
        mock_checker.blocking_failures.return_value = []

        mock_session = MagicMock()
        mock_session.__enter__ = lambda s: s
        mock_session.__exit__ = MagicMock(return_value=False)

        call_count = [0]

        def mock_run(cmd, **kw):
            call_count[0] += 1
            m = MagicMock()
            if "capture" in cmd:
                m.returncode = 1
                m.stderr = "capture failed"
            else:
                m.returncode = 1  # resume also fails
                m.stderr = "resume failed"
            return m

        cfg = MigrationConfig.simple(hostname="h")
        t = Teleporter(pid=1, config=cfg)

        with patch("wraith.teleporter.RemoteSession", return_value=mock_session), \
             patch("wraith.teleporter.PreflightChecker", return_value=mock_checker), \
             patch("wraith.teleporter.subprocess.run", side_effect=mock_run), \
             patch("wraith.teleporter.tempfile.mkstemp", return_value=(0, "/tmp/x.pb")), \
             patch("os.close"), patch("os.path.exists", return_value=False), \
             patch("os.unlink"):

            with pytest.raises(RollbackError) as exc_info:
                t.migrate()

        assert "capture failed" in str(exc_info.value)
        assert "resume failed" in str(exc_info.value)
        assert t.state == MigrationState.FAILED
