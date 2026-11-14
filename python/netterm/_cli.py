"""Finding and running the headless binary, the package's only way to the
workspace and the credential store."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Optional, Sequence

try:
    from . import _build  # written into the wheel by scripts/build-wheel.mjs
except ImportError:  # the source tree, not an installed wheel
    _build = None  # type: ignore[assignment]

#: The variable naming the binary (the app sets it for scripts it starts).
CLI_VARIABLE = "NETTERM_CLI"
#: The binary's name, without ``.exe``.
CLI_NAME: str = getattr(_build, "CLI_NAME", "netterm-cli")
#: The app's name, as the installer names its folder (``None`` in the
#: source tree, which then has no default location).
PRODUCT_NAME: Optional[str] = getattr(_build, "PRODUCT_NAME", None)
#: The version of the binary's JSON this package reads.
FORMAT = 1


class NettermError(Exception):
    """Something the headless binary could not do, or that went wrong
    talking to it. The message says why."""


class CliNotFound(NettermError):
    """The headless binary is in none of the places looked at."""


class CliFailed(NettermError):
    """The headless binary refused or failed. ``status`` is its exit
    status, ``reason`` what it said."""

    def __init__(self, message: str, status: int, reason: str) -> None:
        super().__init__(message)
        self.status = status
        self.reason = reason


def _executable(name: str) -> str:
    return f"{name}.exe" if sys.platform == "win32" else name


def default_location() -> Optional[Path]:
    """Where the installer puts the binary: on Windows, beside the app in
    ``%LOCALAPPDATA%\\<app name>`` (the per-user install). ``None``
    elsewhere, where the binary is looked for on ``PATH`` (the Linux
    packages install it in ``/usr/bin``; the AppImage needs ``NETTERM_CLI``)."""
    if sys.platform != "win32" or PRODUCT_NAME is None:
        return None
    local = os.environ.get("LOCALAPPDATA")
    if not local:
        return None
    return Path(local) / PRODUCT_NAME / _executable(CLI_NAME)


def find_cli() -> Path:
    """The headless binary: ``NETTERM_CLI`` if set, else the default
    install location, else ``PATH``. Raises :class:`CliNotFound` saying
    where it looked."""
    configured = os.environ.get(CLI_VARIABLE, "").strip()
    if configured:
        path = Path(configured)
        if path.is_file():
            return path
        raise CliNotFound(
            f"{CLI_VARIABLE} is set to {configured}, which is not a file: "
            f"point it at {_executable(CLI_NAME)}"
        )
    tried = [f"{CLI_VARIABLE} (not set)"]
    default = default_location()
    if default is not None:
        if default.is_file():
            return default
        tried.append(str(default))
    found = shutil.which(CLI_NAME)
    if found:
        return Path(found)
    tried.append(f"{CLI_NAME} on PATH")
    app = PRODUCT_NAME or "the app"
    raise CliNotFound(
        f"cannot find {_executable(CLI_NAME)}, the headless binary that comes "
        f"with {app} (looked at: {'; '.join(tried)}). Set {CLI_VARIABLE} to its "
        f"path: it is installed beside the app."
    )


def run(args: Sequence[str]) -> Any:
    """Runs the binary with ``args`` and returns the JSON it printed.

    The terminal stays the binary's: with the app's vault locked or closed
    it asks for the master password there, hidden, never through this
    process. What it prints goes nowhere but the returned value."""
    cli = find_cli()
    try:
        done = subprocess.run(
            [str(cli), *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as error:
        raise NettermError(f"cannot start {cli}: {error}") from error
    reason = done.stderr.decode("utf-8", errors="replace").strip()
    if done.returncode != 0:
        prefix = f"{CLI_NAME}: error: "
        lines = [line.removeprefix(prefix) for line in reason.splitlines()]
        said = " ".join(lines) or f"it exited with status {done.returncode}"
        raise CliFailed(said, done.returncode, reason)
    try:
        answer = json.loads(done.stdout.decode("utf-8"))
    except ValueError as error:
        raise NettermError(f"{cli} printed something that is not JSON: {error}") from None
    if not isinstance(answer, dict) or answer.get("format") != FORMAT:
        found = answer.get("format") if isinstance(answer, dict) else None
        raise NettermError(
            f"{cli} speaks format {found}, this package reads format {FORMAT}: "
            "install the package that ships with that version of the app"
        )
    return answer
