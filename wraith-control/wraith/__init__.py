"""
Wraith — Process Teleportation Engine

Public API::

    from wraith import Teleporter, MigrationConfig, TeleportError

    cfg    = MigrationConfig.simple(hostname="worker-2", ssh_key="~/.ssh/id_rsa")
    result = Teleporter(pid=12345, config=cfg).migrate()
    print(result)
"""

from wraith.config import (
    DestinationConfig,
    CaptureConfig,
    TransferConfig,
    RestoreConfig,
    MigrationConfig,
)
from wraith.exceptions import (
    WraithError,
    PreflightError,
    CaptureError,
    TransferError,
    RestoreError,
    RollbackError,
)
from wraith.teleporter import Teleporter, MigrationResult, MigrationState

# Convenience alias matching the exception name in phase5.md
TeleportError = WraithError

__version__ = "0.1.0"

__all__ = [
    # Configuration
    "DestinationConfig",
    "CaptureConfig",
    "TransferConfig",
    "RestoreConfig",
    "MigrationConfig",
    # Orchestration
    "Teleporter",
    "MigrationResult",
    "MigrationState",
    # Exceptions
    "WraithError",
    "TeleportError",
    "PreflightError",
    "CaptureError",
    "TransferError",
    "RestoreError",
    "RollbackError",
]
