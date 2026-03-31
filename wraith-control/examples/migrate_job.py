#!/usr/bin/env python3
"""
Example: Migrate a long-running compute job to another machine.

This shows how to use the Wraith Python API directly, without the CLI.
The same logic is what `wraith migrate` executes under the hood.

Usage:
    python migrate_job.py --pid 12345 --dest worker-2.example.com --key ~/.ssh/id_rsa

Prerequisites on the destination machine:
    - wraith-restorer installed in PATH
    - wraith-receiver installed in PATH
    - SSH access with the provided key
    - ptrace_scope = 0 (or run as root)
"""

from __future__ import annotations

import argparse
import sys

from wraith import (
    MigrationConfig,
    Teleporter,
    TeleportError,
    RollbackError,
)


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Migrate a running process to another machine using Wraith."
    )
    p.add_argument("--pid",  required=True, type=int, help="PID of the process to migrate")
    p.add_argument("--dest", required=True,           help="Destination hostname or IP")
    p.add_argument("--user", default="root",          help="SSH username (default: root)")
    p.add_argument("--key",  default=None,            help="SSH private key path")
    p.add_argument("--port", default=22, type=int,    help="SSH port (default: 22)")
    p.add_argument(
        "--verify-only",
        action="store_true",
        help="Restore but do NOT kill the source process (smoke test)",
    )
    return p.parse_args()


def main() -> int:
    args = parse_args()

    print(f"Wraith: migrating PID {args.pid} → {args.user}@{args.dest}:{args.port}")
    if args.verify_only:
        print("  (verify-only mode — source will survive)")

    # Build config with defaults.
    cfg = MigrationConfig.simple(
        hostname=args.dest,
        port=args.port,
        username=args.user,
        ssh_key=args.key,
    )

    t = Teleporter(pid=args.pid, config=cfg)

    try:
        result = t.migrate(verify_only=args.verify_only)

    except RollbackError as e:
        print(f"\nCRITICAL: {e}", file=sys.stderr)
        print(
            f"\nACTION REQUIRED: Unfreeze source PID {args.pid}:\n"
            f"  wraith-capturer resume --pid {args.pid}",
            file=sys.stderr,
        )
        return 3

    except TeleportError as e:
        print(f"\nMigration failed: {e}", file=sys.stderr)
        print("  Source process was rolled back (unfrozen) automatically.", file=sys.stderr)
        return 1

    # Success.
    print()
    print(f"  Migration ID : {result.migration_id}")
    print(f"  Source PID   : {result.source_pid}  (terminated)")
    print(f"  Dest PID     : {result.destination_pid} on {result.destination_host}")
    print(f"  Snapshot     : {result.snapshot_size_bytes // 1024 // 1024} MB")
    print(f"  Elapsed      : {result.elapsed_s:.1f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
