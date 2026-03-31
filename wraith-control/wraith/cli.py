"""
Wraith command-line interface.

Commands:
  migrate   — Full cross-machine process migration (capture → transfer → restore)
  check     — Run pre-flight checks only (no migration)
  capture   — Capture a process snapshot locally (debug)
  transfer  — Transfer an existing snapshot to a destination (debug)
  restore   — Restore a snapshot on this machine (debug)

Usage:
  wraith migrate --pid 12345 --destination worker-2.example.com --key ~/.ssh/id_rsa
  wraith check   --pid 12345 --destination worker-2.example.com
  wraith capture --pid 12345 --output /tmp/snap.pb
  wraith transfer --snapshot /tmp/snap.pb --destination worker-2.example.com
  wraith restore --snapshot /tmp/snap.pb
"""

from __future__ import annotations

import os
import subprocess
import sys
from typing import Optional

import click

from wraith.config import (
    CaptureConfig,
    DestinationConfig,
    MigrationConfig,
    RestoreConfig,
    TransferConfig,
)
from wraith.exceptions import (
    PreflightError,
    RollbackError,
    WraithError,
)
from wraith.logging import setup_logging


# ── Root group ────────────────────────────────────────────────────────────────

@click.group()
@click.option("-v", "--verbose", is_flag=True, help="Enable debug output")
@click.option("--log-file", metavar="PATH", help="Write JSON log to file")
@click.version_option(package_name="wraith-control")
@click.pass_context
def cli(ctx: click.Context, verbose: bool, log_file: Optional[str]) -> None:
    """Wraith — Process Teleportation Engine.

    Migrates a running Linux process (registers + memory + file descriptors)
    from one machine to another with no application changes required.

    Use --verbose for detailed output; --log-file for structured JSON logs.
    """
    setup_logging(verbose=verbose, log_file=log_file)
    ctx.ensure_object(dict)


# ── migrate ───────────────────────────────────────────────────────────────────

@cli.command()
@click.option("--pid",         "-p", required=True, type=int,      help="Source process PID")
@click.option("--destination", "-d", required=True,                help="Destination hostname or IP")
@click.option("--username",    "-u", default="root", show_default=True, help="SSH username")
@click.option("--key",         "-k", default=None,                 help="SSH private key path")
@click.option("--port",              default=22,    show_default=True, help="SSH port")
@click.option("--transfer-port",     default=9999,  show_default=True, help="Wraith receiver port")
@click.option("--verify-only",       is_flag=True,  help="Restore but do NOT kill source (smoke-test mode)")
@click.option("--strict-fds",        is_flag=True,  help="Fail if any file descriptor cannot be restored")
@click.option("--keep-snapshot",     is_flag=True,  help="Keep snapshot files after migration")
@click.option("--max-memory-gb",     default=64.0, show_default=True, type=float,
              help="Refuse to capture processes larger than this (GB RSS)")
@click.option("--timeout",           default=300,  show_default=True,
              help="Transfer timeout in seconds")
def migrate(
    pid: int,
    destination: str,
    username: str,
    key: Optional[str],
    port: int,
    transfer_port: int,
    verify_only: bool,
    strict_fds: bool,
    keep_snapshot: bool,
    max_memory_gb: float,
    timeout: int,
) -> None:
    """Migrate a running process to another machine.

    The source process is frozen for the duration of the migration and
    resumed automatically if anything fails (rollback is guaranteed).

    Example:

        wraith migrate --pid 12345 --destination worker-2 --key ~/.ssh/id_rsa
    """
    from wraith.teleporter import Teleporter

    cfg = MigrationConfig(
        destination=DestinationConfig(
            hostname=destination,
            port=port,
            username=username,
            ssh_key=_expand_key(key),
            transfer_port=transfer_port,
        ),
        capture=CaptureConfig(max_memory_gb=max_memory_gb),
        transfer=TransferConfig(timeout_s=timeout),
        restore=RestoreConfig(strict_fds=strict_fds, keep_snapshot=keep_snapshot),
    )

    click.echo(f"Migrating PID {pid} → {username}@{destination}:{port}")
    if verify_only:
        click.echo("  (verify-only mode: source will NOT be terminated)")

    t = Teleporter(pid=pid, config=cfg)
    try:
        result = t.migrate(verify_only=verify_only)
        click.echo()
        click.secho("  Migration successful!", fg="green")
        click.echo(f"  Source  : PID {result.source_pid} on this machine")
        click.echo(f"  Dest    : PID {result.destination_pid} on {result.destination_host}")
        click.echo(f"  Elapsed : {result.elapsed_s:.1f}s")
        click.echo(f"  Size    : {result.snapshot_size_bytes // 1024 // 1024} MB")
        click.echo(f"  ID      : {result.migration_id}")

    except PreflightError as e:
        click.secho(f"\n  Pre-flight failed — source process untouched", fg="yellow")
        click.echo(f"  {e}")
        sys.exit(1)

    except RollbackError as e:
        click.secho(f"\n  CRITICAL: Migration failed AND rollback failed!", fg="red", bold=True)
        click.echo(f"  {e}")
        click.secho(
            f"\n  ACTION REQUIRED: Run the following on the source machine to unfreeze PID {pid}:",
            fg="red",
        )
        click.echo(f"    wraith-capturer resume --pid {pid}")
        sys.exit(3)

    except WraithError as e:
        click.secho(f"\n  Migration failed — source process has been unfrozen (rolled back)", fg="yellow")
        click.echo(f"  {e}")
        sys.exit(1)


# ── check ─────────────────────────────────────────────────────────────────────

@cli.command()
@click.option("--pid",         "-p", required=True, type=int, help="Source process PID")
@click.option("--destination", "-d", required=True,           help="Destination hostname or IP")
@click.option("--username",    "-u", default="root", show_default=True)
@click.option("--key",         "-k", default=None)
@click.option("--port",              default=22,    show_default=True)
def check(pid: int, destination: str, username: str, key: Optional[str], port: int) -> None:
    """Run pre-flight checks without starting a migration.

    Validates that the source process, destination machine, available memory,
    architecture, and installed binaries all meet Wraith's requirements.

    Exit code 0 = all checks passed. Exit code 1 = one or more blocking failures.
    """
    from wraith.checks import PreflightChecker
    from wraith.remote import RemoteSession

    cfg = MigrationConfig.simple(
        hostname=destination,
        port=port,
        username=username,
        ssh_key=_expand_key(key),
    )

    click.echo(f"Running pre-flight checks: PID {pid} → {destination}")

    with RemoteSession(cfg.destination) as sess:
        checker = PreflightChecker(pid=pid, config=cfg, session=sess)
        passed = checker.run_all()

    click.echo()
    click.echo(f"  {'Check':<35} {'Result':<8} Message")
    click.echo(f"  {'─' * 35} {'─' * 8} {'─' * 40}")
    for r in checker.results:
        colour  = "green" if r.passed else ("red" if r.blocking else "yellow")
        symbol  = "PASS" if r.passed else ("FAIL" if r.blocking else "WARN")
        click.echo(
            f"  {r.name:<35} "
            + click.style(f"{symbol:<8}", fg=colour)
            + f" {r.message}"
        )

    click.echo()
    if passed:
        click.secho("  All checks passed — ready to migrate.", fg="green")
    else:
        failures = checker.blocking_failures()
        click.secho(f"  {len(failures)} blocking failure(s) — cannot migrate.", fg="red")
        sys.exit(1)


# ── capture (debug) ───────────────────────────────────────────────────────────

@cli.command()
@click.option("--pid",    "-p", required=True, type=int, help="Process PID to capture")
@click.option("--output", "-o", default="snapshot.pb", show_default=True,
              help="Output path for the snapshot file")
@click.option("--capturer", default="wraith-capturer", show_default=True,
              help="Path to wraith-capturer binary")
def capture(pid: int, output: str, capturer: str) -> None:
    """Capture a process snapshot to a file (debug subcommand).

    This is equivalent to running wraith-capturer directly. Useful for
    inspecting or archiving a snapshot without a full migration.
    """
    click.echo(f"Capturing PID {pid} → {output}")
    try:
        result = subprocess.run(
            [capturer, "capture", "--pid", str(pid), "--output", output],
            check=True,
        )
    except FileNotFoundError:
        click.secho(f"Error: {capturer!r} not found. Check PATH or --capturer.", fg="red")
        sys.exit(1)
    except subprocess.CalledProcessError as e:
        click.secho(f"Capture failed (exit {e.returncode})", fg="red")
        sys.exit(1)

    size_mb = os.path.getsize(output) / 1024 / 1024
    click.secho(f"  Saved: {output} ({size_mb:.1f} MB)", fg="green")


# ── transfer (debug) ──────────────────────────────────────────────────────────

@cli.command()
@click.option("--snapshot",    "-s", required=True, help="Local snapshot file path")
@click.option("--destination", "-d", required=True, help="Destination address:port")
@click.option("--transmitter", default="wraith-transmitter", show_default=True)
@click.option("--retries",     default=3, show_default=True)
@click.option("--timeout",     default=300, show_default=True)
def transfer(snapshot: str, destination: str, transmitter: str, retries: int, timeout: int) -> None:
    """Transfer an existing snapshot to a destination (debug subcommand).

    Requires wraith-receiver to be listening on the destination first.
    The destination format is host:port (e.g. worker-2:9999).
    """
    click.echo(f"Transmitting {snapshot} → {destination}")
    try:
        subprocess.run(
            [
                transmitter,
                "--snapshot", snapshot,
                "--dest",     destination,
                "--retries",  str(retries),
                "--timeout",  str(timeout),
            ],
            check=True,
        )
        click.secho("  Transfer complete.", fg="green")
    except FileNotFoundError:
        click.secho(f"Error: {transmitter!r} not found.", fg="red")
        sys.exit(1)
    except subprocess.CalledProcessError as e:
        click.secho(f"Transfer failed (exit {e.returncode})", fg="red")
        sys.exit(1)


# ── restore (debug) ───────────────────────────────────────────────────────────

@cli.command()
@click.option("--snapshot",   "-s", required=True, help="Snapshot file path")
@click.option("--restorer",   default="wraith-restorer", show_default=True)
@click.option("--strict-fds", is_flag=True, help="Fail on any FD restoration error")
def restore(snapshot: str, restorer: str, strict_fds: bool) -> None:
    """Restore a snapshot to a new process on this machine (debug subcommand).

    This calls wraith-restorer directly and prints its output.
    Must be run as root or with CAP_SYS_PTRACE.
    """
    click.echo(f"Restoring {snapshot}")
    cmd = [restorer, "--snapshot", snapshot]
    if strict_fds:
        cmd.append("--strict-fds")
    try:
        subprocess.run(cmd, check=True)
    except FileNotFoundError:
        click.secho(f"Error: {restorer!r} not found.", fg="red")
        sys.exit(1)
    except subprocess.CalledProcessError as e:
        click.secho(f"Restore failed (exit {e.returncode})", fg="red")
        sys.exit(1)


# ── Helpers ───────────────────────────────────────────────────────────────────

def _expand_key(path: Optional[str]) -> Optional[str]:
    """Expand ~ in SSH key path."""
    return os.path.expanduser(path) if path else None
