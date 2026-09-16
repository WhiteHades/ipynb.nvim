#!/usr/bin/env python3
"""Install the isolated Python runtime used by ipynb.nvim."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path


DISPLAY_NAME = "Python (ipynb.nvim)"


def python_path(venv: Path) -> Path:
    """Return the interpreter path for a virtual environment."""

    candidates = (venv / "bin" / "python", venv / "Scripts" / "python.exe")
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise RuntimeError(
        f"{venv} exists but has no Python interpreter; refusing to recreate it. "
        "Remove it yourself if it is not a virtual environment, then retry."
    )


def run(command: list[str]) -> None:
    """Run a command while keeping failures actionable for users."""

    print("+", " ".join(command))
    subprocess.run(command, check=True)


def creator_python(requested: str | None) -> str:
    if requested is None:
        return sys.executable

    resolved = shutil.which(requested)
    if resolved:
        return resolved

    candidate = Path(requested).expanduser()
    if candidate.is_file():
        return str(candidate)

    raise RuntimeError(f"Python interpreter not found: {requested}")


def create_environment(venv: Path, requested_python: str | None) -> None:
    uv = shutil.which("uv")
    if uv:
        command = [uv, "venv"]
        if requested_python:
            command.extend(["--python", requested_python])
        command.append(str(venv))
    else:
        command = [creator_python(requested_python), "-m", "venv", str(venv)]
    run(command)


def install_requirements(venv: Path, requirements: Path) -> Path:
    interpreter = python_path(venv)
    uv = shutil.which("uv")
    if uv:
        run([uv, "pip", "install", "--python", str(interpreter), "-r", str(requirements)])
    else:
        run([str(interpreter), "-m", "pip", "install", "-r", str(requirements)])
    return interpreter


def install_kernel(venv: Path, interpreter: Path) -> None:
    run(
        [
            str(interpreter),
            "-m",
            "ipykernel",
            "install",
            "--prefix",
            str(venv),
            "--name",
            "python3",
            "--display-name",
            DISPLAY_NAME,
        ]
    )


def install_runtime(venv: Path, requirements: Path, requested_python: str | None = None) -> Path:
    """Create or reuse *venv*, install dependencies, and register its kernel."""

    venv = venv.expanduser().resolve()
    requirements = requirements.expanduser().resolve()
    if not requirements.is_file():
        raise RuntimeError(f"Requirements file not found: {requirements}")

    if venv.exists():
        interpreter = python_path(venv)
        print(f"Reusing existing Python environment: {venv}")
    else:
        venv.parent.mkdir(parents=True, exist_ok=True)
        create_environment(venv, requested_python)
        interpreter = python_path(venv)

    install_requirements(venv, requirements)
    install_kernel(venv, interpreter)
    print(f"ipynb.nvim Python runtime is ready: {interpreter}")
    return interpreter


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--venv", required=True, type=Path, help="virtual-environment path")
    parser.add_argument("--python", dest="requested_python", help="Python used to create a new environment")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    requirements = Path(__file__).resolve().parents[1] / "requirements.txt"
    try:
        install_runtime(args.venv, requirements, args.requested_python)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"ipynb.nvim setup failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
