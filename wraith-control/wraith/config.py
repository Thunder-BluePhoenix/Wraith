"""
Configuration dataclasses for Wraith migration.

All config objects are immutable after construction; validation is done
in __post_init__ so callers get clear errors immediately, not mid-migration.
"""

from __future__ import annotations

import os
from dataclasses import dataclass, field
from typing import Optional


@dataclass(frozen=True)
class DestinationConfig:
    """
    Connection parameters for the destination machine.

    The destination machine must have:
      - wraith-restorer installed (see wraith_restorer_path)
      - wraith-receiver listening or willing to be started
      - SSH access with the given credentials
    """

    hostname: str
    port: int = 22
    username: str = "root"
    ssh_key: Optional[str] = None          # Path to private key file; None = agent/default
    ssh_timeout_s: int = 10
    transfer_port: int = 9999              # Port the Go receiver listens on
    wraith_capturer_path: str = "wraith-capturer"
    wraith_restorer_path: str = "wraith-restorer"
    wraith_receiver_path: str = "wraith-receiver"
    wraith_transmitter_path: str = "wraith-transmitter"
    remote_snapshot_dir: str = "/tmp"     # Where the receiver writes the snapshot

    def __post_init__(self) -> None:
        if not self.hostname:
            raise ValueError("DestinationConfig.hostname must not be empty")
        if not (1 <= self.port <= 65535):
            raise ValueError(f"DestinationConfig.port {self.port!r} is out of range")
        if self.ssh_key and not os.path.exists(self.ssh_key):
            raise ValueError(f"SSH key file not found: {self.ssh_key!r}")

    @property
    def transfer_addr(self) -> str:
        """Address string passed to wraith-transmitter: host:port."""
        return f"{self.hostname}:{self.transfer_port}"

    @property
    def ssh_addr(self) -> str:
        return f"{self.username}@{self.hostname}:{self.port}"


@dataclass(frozen=True)
class CaptureConfig:
    """Tuning parameters for the capture phase (wraith-capturer)."""

    freeze_timeout_s: int = 30     # How long to wait for ptrace attach to complete
    max_memory_gb: float = 64.0    # Refuse to capture processes larger than this
    skip_validation: bool = False  # Skip RIP/RSP sanity checks (for debugging only)

    def __post_init__(self) -> None:
        if self.freeze_timeout_s <= 0:
            raise ValueError("freeze_timeout_s must be positive")
        if self.max_memory_gb <= 0:
            raise ValueError("max_memory_gb must be positive")


@dataclass(frozen=True)
class TransferConfig:
    """Tuning parameters for the Go transport phase."""

    timeout_s: int = 300           # Total transfer timeout
    retry_count: int = 3           # Per-block retry limit
    receiver_start_timeout_s: int = 15  # How long to wait for receiver to start listening

    def __post_init__(self) -> None:
        if self.timeout_s <= 0:
            raise ValueError("timeout_s must be positive")
        if self.retry_count < 0:
            raise ValueError("retry_count must be >= 0")


@dataclass(frozen=True)
class RestoreConfig:
    """Tuning parameters for the restore phase (wraith-restorer)."""

    startup_timeout_s: int = 60   # How long to wait for restorer to complete
    strict_fds: bool = False       # Pass --strict-fds to wraith-restorer
    keep_snapshot: bool = False    # Keep remote snapshot file after restore


@dataclass(frozen=True)
class MigrationConfig:
    """
    Top-level configuration object passed to Teleporter.

    Build with the factory methods or supply each sub-config directly.

    Example::

        cfg = MigrationConfig.simple(
            hostname="worker-2.example.com",
            ssh_key="~/.ssh/id_rsa",
        )
    """

    destination: DestinationConfig
    capture: CaptureConfig = field(default_factory=CaptureConfig)
    transfer: TransferConfig = field(default_factory=TransferConfig)
    restore: RestoreConfig = field(default_factory=RestoreConfig)

    @classmethod
    def simple(
        cls,
        hostname: str,
        *,
        port: int = 22,
        username: str = "root",
        ssh_key: Optional[str] = None,
        transfer_port: int = 9999,
    ) -> "MigrationConfig":
        """Create a config with all defaults — the common case."""
        return cls(
            destination=DestinationConfig(
                hostname=hostname,
                port=port,
                username=username,
                ssh_key=ssh_key,
                transfer_port=transfer_port,
            )
        )
