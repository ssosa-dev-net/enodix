"""Finding and running the headless binary, the package's only way to the
workspace and the credential store."""

from __future__ import annotations

import json
import os
import shutil
import re
import subprocess
import sys
import threading
import warnings
from pathlib import Path
from typing import Any, List, Optional, Sequence, Set

try:
    from . import _build  # written into the wheel by scripts/build-wheel.mjs
except ImportError:  # the source tree, not an installed wheel
    _build = None  # type: ignore[assignment]

#: The variable naming the binary (the app sets it for scripts it starts).
CLI_VARIABLE = "ENODIX_CLI"
#: The binary's name, without ``.exe``.
CLI_NAME: str = getattr(_build, "CLI_NAME", "enodix-cli")
#: The app's name, as the installer names its folder (``None`` in the
#: source tree, which then has no default location).
PRODUCT_NAME: Optional[str] = getattr(_build, "PRODUCT_NAME", None)
#: The version of the binary's JSON this package reads.
FORMAT = 1
#: This package's version as the wheel has it (``None`` in the source tree,
#: which is never compared with the binary's).
PACKAGE_VERSION: Optional[str] = getattr(_build, "VERSION", None)
#: Ends the binary's options: what follows is a value even when it starts
#: with ``-``. Passed only when a value needs it, so that a binary older
#: than ``--`` still runs every command line it understood.
END_OF_OPTIONS = "--"

#: The binaries whose version was compared with the package's already.
_COMPARED: Set[str] = set()
_COMPARED_LOCK = threading.Lock()


class EnodixError(Exception):
    """Something the headless binary could not do, or that went wrong
    talking to it. The message says why."""


class CliNotFound(EnodixError):
    """The headless binary is in none of the places looked at."""


class VersionWarning(UserWarning):
    """The headless binary is another version than this package: it was
    left behind when the app was upgraded (or the other way round). Things
    may still work; the message says what to install."""


class CliFailed(EnodixError):
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
    packages install it in ``/usr/bin``; the AppImage needs ``ENODIX_CLI``)."""
    if sys.platform != "win32" or PRODUCT_NAME is None:
        return None
    local = os.environ.get("LOCALAPPDATA")
    if not local:
        return None
    return Path(local) / PRODUCT_NAME / _executable(CLI_NAME)


def find_cli() -> Path:
    """The headless binary: ``ENODIX_CLI`` if set, else the default
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


def python_version(version: str) -> str:
    """An app version as Python writes it (PEP 440, as the wheel is named:
    ``1.2.3-beta.1`` -> ``1.2.3b1``, build details dropped)."""
    match = re.fullmatch(r"(\d+\.\d+\.\d+)(?:-(alpha|beta|rc)\.(\d+))?(?:\+[0-9A-Za-z.-]+)?", version)
    if not match:
        return version
    release, stage, number = match.groups()
    short = {"alpha": "a", "beta": "b", "rc": "rc"}
    return f"{release}{short[stage]}{number}" if stage else release


def compare_version(cli: Path) -> None:
    """Warns (:class:`VersionWarning`, once per binary and process) when the
    binary's version (``--version``) is not this package's. Nothing is
    refused: the JSON format is what decides whether they understand each
    other. Says nothing when either version cannot be told (the source
    tree, a binary that does not answer)."""
    if PACKAGE_VERSION is None:
        return
    with _COMPARED_LOCK:
        if str(cli) in _COMPARED:
            return
        _COMPARED.add(str(cli))
    try:
        done = subprocess.run(
            [str(cli), "--version"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return
    words = done.stdout.decode("utf-8", errors="replace").split()
    if done.returncode != 0 or len(words) < 2:
        return
    binary = python_version(words[1])
    if binary == PACKAGE_VERSION:
        return
    app = PRODUCT_NAME or "the app"
    warnings.warn(
        VersionWarning(
            f"{cli} is version {binary} of {app}'s headless binary, and this enodix "
            f"package is version {PACKAGE_VERSION}: install the package that ships with "
            f"the app (pip install the wheel in the python folder beside it), or point "
            f"{CLI_VARIABLE} at the binary of this package's version"
        ),
        stacklevel=4,
    )


def values(*given: str) -> List[str]:
    """Values for the binary's command line (a workspace folder, a host id, a
    secret's name), behind ``--`` if one of them starts with ``-``."""
    if any(value.startswith("-") for value in given):
        return [END_OF_OPTIONS, *given]
    return list(given)


def option(name: str, value: str) -> List[str]:
    """An option and its value, as ``--name=<value>`` when the value starts
    with ``-``."""
    if value.startswith("-"):
        return [f"{name}={value}"]
    return [name, value]


def run(args: Sequence[str]) -> Any:
    """Runs the binary with ``args`` and returns the JSON it printed.

    The terminal stays the binary's: with the app's vault locked or closed
    it asks for the master password there, hidden, never through this
    process. What it prints goes nowhere but the returned value."""
    cli = find_cli()
    compare_version(cli)
    try:
        done = subprocess.run(
            [str(cli), *args],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as error:
        raise EnodixError(f"cannot start {cli}: {error}") from error
    reason = done.stderr.decode("utf-8", errors="replace").strip()
    if done.returncode != 0:
        prefix = f"{CLI_NAME}: error: "
        lines = [line.removeprefix(prefix) for line in reason.splitlines()]
        said = " ".join(lines) or f"it exited with status {done.returncode}"
        raise CliFailed(said, done.returncode, reason)
    try:
        answer = json.loads(done.stdout.decode("utf-8"))
    except ValueError as error:
        raise EnodixError(f"{cli} printed something that is not JSON: {error}") from None
    if not isinstance(answer, dict) or answer.get("format") != FORMAT:
        found = answer.get("format") if isinstance(answer, dict) else None
        raise EnodixError(
            f"{cli} speaks format {found}, this package reads format {FORMAT}: "
            "install the package that ships with that version of the app"
        )
    return answer
