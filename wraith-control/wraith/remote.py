"""
SSH session management for Wraith orchestration.

Wraps paramiko to provide:
  - One persistent connection per destination (reused across all phases)
  - exec_command with stdout/stderr capture and exit-code checking
  - SCP-style file upload for the snapshot
  - Background command launch (for starting wraith-receiver)
  - Port-forward context manager (future use)

All methods raise specific WraithError subtypes rather than paramiko exceptions,
so callers don't need to import paramiko.
"""

from __future__ import annotations

import logging
import os
import socket
import stat
from contextlib import contextmanager
from typing import Iterator, Optional, Tuple

from wraith.config import DestinationConfig
from wraith.exceptions import WraithError

log = logging.getLogger("wraith.remote")


class CommandError(WraithError):
    """A remote command exited with non-zero status."""

    def __init__(self, cmd: str, exit_code: int, stderr: str) -> None:
        self.exit_code = exit_code
        self.stderr_output = stderr
        super().__init__(
            f"Remote command failed (exit {exit_code}): {cmd!r}\n  stderr: {stderr.strip()}"
        )


class RemoteSession:
    """
    A persistent SSH session to a destination machine.

    Open with the context manager::

        with RemoteSession(dest) as sess:
            sess.exec("uname -m")
            sess.upload("/local/snapshot.pb", "/tmp/snapshot.pb")

    Or use `connect()` / `close()` for long-lived sessions managed by Teleporter.
    """

    def __init__(self, dest: DestinationConfig) -> None:
        self._dest = dest
        self._client: Optional["paramiko.SSHClient"] = None  # type: ignore[name-defined]

    # ── Connection lifecycle ──────────────────────────────────────────────────

    def connect(self) -> None:
        """Open the SSH connection. Idempotent."""
        if self._client is not None:
            return

        try:
            import paramiko
        except ImportError as e:
            raise WraithError(
                "paramiko is not installed",
                hint="pip install paramiko>=3.0",
            ) from e

        dest = self._dest
        client = paramiko.SSHClient()
        client.set_missing_host_key_policy(paramiko.AutoAddPolicy())

        try:
            client.connect(
                hostname=dest.hostname,
                port=dest.port,
                username=dest.username,
                key_filename=dest.ssh_key or None,
                timeout=dest.ssh_timeout_s,
                banner_timeout=dest.ssh_timeout_s,
                auth_timeout=dest.ssh_timeout_s,
            )
        except paramiko.AuthenticationException as e:
            raise WraithError(
                f"SSH auth failed for {dest.ssh_addr}: {e}",
                hint="Check SSH key path and that it is authorised on the destination",
            ) from e
        except (socket.timeout, socket.error, paramiko.SSHException) as e:
            raise WraithError(
                f"Cannot connect to {dest.ssh_addr}: {e}",
                hint="Check hostname, port, and that sshd is running",
            ) from e

        self._client = client
        log.debug("SSH connected to %s", dest.ssh_addr)

    def close(self) -> None:
        """Close the SSH connection. Safe to call multiple times."""
        if self._client is not None:
            try:
                self._client.close()
            except Exception:
                pass
            self._client = None
            log.debug("SSH disconnected from %s", self._dest.ssh_addr)

    def __enter__(self) -> "RemoteSession":
        self.connect()
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    # ── Command execution ─────────────────────────────────────────────────────

    def exec(
        self,
        cmd: str,
        timeout: Optional[int] = None,
        check: bool = True,
        env: Optional[dict] = None,
    ) -> Tuple[str, str, int]:
        """
        Run a command on the destination machine.

        Returns:
            (stdout, stderr, exit_code) — all text decoded as UTF-8.

        Raises:
            CommandError: if check=True and exit code is non-zero.
        """
        self._ensure_connected()
        effective_timeout = timeout or self._dest.ssh_timeout_s

        log.debug("Remote exec: %s", cmd)
        stdin, stdout_ch, stderr_ch = self._client.exec_command(  # type: ignore[union-attr]
            cmd,
            timeout=effective_timeout,
            environment=env,
        )
        stdin.close()

        out = stdout_ch.read().decode("utf-8", errors="replace")
        err = stderr_ch.read().decode("utf-8", errors="replace")
        code = stdout_ch.channel.recv_exit_status()

        log.debug("Exit %d | stdout=%r | stderr=%r", code, out[:200], err[:200])

        if check and code != 0:
            raise CommandError(cmd, code, err)

        return out, err, code

    def exec_background(self, cmd: str) -> None:
        """
        Start a command on the destination and return immediately.

        The command continues running after this call returns. There is no way
        to retrieve its output or wait for it — use for fire-and-forget daemons
        like wraith-receiver.
        """
        self._ensure_connected()
        log.debug("Remote background exec: %s", cmd)
        transport = self._client.get_transport()  # type: ignore[union-attr]
        channel = transport.open_session()  # type: ignore[union-attr]
        channel.exec_command(f"nohup {cmd} </dev/null >/tmp/wraith_receiver.log 2>&1 &")
        channel.close()

    # ── File transfer ─────────────────────────────────────────────────────────

    def upload(self, local_path: str, remote_path: str) -> None:
        """
        Upload a local file to the destination via SFTP.

        The file is uploaded with mode 0600 (owner-read-write only) since
        snapshots contain full process memory and must not be world-readable.
        """
        self._ensure_connected()
        size = os.path.getsize(local_path)
        log.debug(
            "Uploading %s → %s:%s (%d bytes)",
            local_path, self._dest.hostname, remote_path, size
        )

        try:
            sftp = self._client.open_sftp()  # type: ignore[union-attr]
            sftp.put(local_path, remote_path)
            sftp.chmod(remote_path, stat.S_IRUSR | stat.S_IWUSR)  # 0600
            sftp.close()
        except Exception as e:
            raise WraithError(
                f"SFTP upload {local_path!r} → {remote_path!r} failed: {e}",
                hint="Check that the destination directory exists and is writable",
            ) from e

    def download(self, remote_path: str, local_path: str) -> None:
        """Download a file from the destination (used for log retrieval)."""
        self._ensure_connected()
        try:
            sftp = self._client.open_sftp()  # type: ignore[union-attr]
            sftp.get(remote_path, local_path)
            sftp.close()
        except Exception as e:
            raise WraithError(
                f"SFTP download {remote_path!r} → {local_path!r} failed: {e}"
            ) from e

    def file_exists(self, remote_path: str) -> bool:
        """Return True if the file exists on the destination."""
        try:
            out, _, code = self.exec(f"test -f {remote_path!r}", check=False)
            return code == 0
        except Exception:
            return False

    def read_remote_text(self, remote_path: str) -> str:
        """Read a small text file from the destination."""
        out, _, _ = self.exec(f"cat {remote_path!r}")
        return out

    # ── Internal ─────────────────────────────────────────────────────────────

    def _ensure_connected(self) -> None:
        if self._client is None:
            raise WraithError(
                "RemoteSession is not connected — call connect() first"
            )
