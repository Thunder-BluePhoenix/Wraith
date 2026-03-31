"""
Logging setup for Wraith orchestration.

Two handlers:
  - Console: human-readable with level prefix
  - JSON file: structured log for post-mortem analysis

Usage::

    from wraith.logging import setup_logging
    setup_logging(verbose=True, log_file="/var/log/wraith/migrate.json")
"""

from __future__ import annotations

import json
import logging
import sys
import time
from typing import Optional


class _JsonFormatter(logging.Formatter):
    """Emit one JSON object per line with standard Wraith fields."""

    def format(self, record: logging.LogRecord) -> str:
        obj = {
            "ts":      time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(record.created)),
            "level":   record.levelname,
            "logger":  record.name,
            "msg":     record.getMessage(),
        }
        if record.exc_info:
            obj["exc"] = self.formatException(record.exc_info)
        # Attach any extra fields set by callers (e.g. log.info("...", extra={"pid": 1234}))
        for key, val in record.__dict__.items():
            if key not in logging.LogRecord.__dict__ and not key.startswith("_"):
                try:
                    json.dumps(val)  # only include JSON-serialisable extras
                    obj[key] = val
                except (TypeError, ValueError):
                    obj[key] = repr(val)
        return json.dumps(obj)


class _ConsoleFormatter(logging.Formatter):
    _COLOURS = {
        "DEBUG":    "\033[90m",   # grey
        "INFO":     "\033[0m",    # default
        "WARNING":  "\033[33m",   # yellow
        "ERROR":    "\033[31m",   # red
        "CRITICAL": "\033[35m",   # magenta
    }
    _RESET = "\033[0m"

    def format(self, record: logging.LogRecord) -> str:
        colour = self._COLOURS.get(record.levelname, "")
        reset  = self._RESET if colour else ""
        prefix = {
            "DEBUG":    "  [dbg]",
            "INFO":     "      ",
            "WARNING":  "  [!] ",
            "ERROR":    "  [x] ",
            "CRITICAL": "  [X] ",
        }.get(record.levelname, "")
        return f"{colour}{prefix} {record.getMessage()}{reset}"


def setup_logging(
    verbose: bool = False,
    log_file: Optional[str] = None,
    migration_id: Optional[str] = None,
) -> None:
    """
    Configure the root logger for Wraith.

    Call once at program startup (CLI entry point).

    Args:
        verbose:      Enable DEBUG messages on console (default: INFO only).
        log_file:     If given, write JSON logs to this path in addition to console.
        migration_id: If given, include in every JSON log record as `migration_id`.
    """
    root = logging.getLogger("wraith")
    root.setLevel(logging.DEBUG)

    # Console handler.
    console = logging.StreamHandler(sys.stderr)
    console.setLevel(logging.DEBUG if verbose else logging.INFO)
    console.setFormatter(_ConsoleFormatter())
    root.addHandler(console)

    # Optional JSON file handler.
    if log_file:
        try:
            fh = logging.FileHandler(log_file, encoding="utf-8")
            fh.setLevel(logging.DEBUG)
            fh.setFormatter(_JsonFormatter())
            root.addHandler(fh)
        except OSError as e:
            root.warning("Could not open log file %r: %s", log_file, e)

    if migration_id:
        # Inject migration_id into all records via a filter.
        class _IdFilter(logging.Filter):
            def filter(self, record: logging.LogRecord) -> bool:
                record.migration_id = migration_id  # type: ignore[attr-defined]
                return True

        root.addFilter(_IdFilter())


def get_logger(name: str) -> logging.Logger:
    """Return a child logger under the `wraith` namespace."""
    return logging.getLogger(f"wraith.{name}")
