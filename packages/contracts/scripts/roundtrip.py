#!/usr/bin/env python3
"""Round-trip every valid fixture through Python, Node, and Rust runtimes."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
VALID = ROOT / "schemas/v1/examples/valid"
INVALID = ROOT / "schemas/v1/examples/invalid"
PY_RUNTIME = ROOT / "packages/contracts/generated/python/roundtrip.py"
NODE_RUNTIME = ROOT / "packages/contracts/generated/typescript/runtime.mjs"
RUST_MANIFEST = ROOT / "packages/contracts/generated/rust/Cargo.toml"


def run(command: list[str], *, expected_success: bool) -> str:
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True)
    if expected_success and result.returncode != 0:
        raise RuntimeError(f"failed: {' '.join(command)}\n{result.stderr}")
    if not expected_success and result.returncode == 0:
        raise RuntimeError(f"invalid fixture unexpectedly passed: {' '.join(command)}")
    return result.stdout.strip()


def runtimes(path: Path) -> list[list[str]]:
    commands = [[sys.executable, str(PY_RUNTIME), str(path)]]
    node = shutil.which("node")
    cargo = shutil.which("cargo")
    if node is None or cargo is None:
        raise RuntimeError("cross-language contract checks require node and cargo")
    commands.append([node, str(NODE_RUNTIME), str(path)])
    commands.append([cargo, "run", "--quiet", "--manifest-path", str(RUST_MANIFEST), "--bin", "roundtrip", "--", str(path)])
    return commands


def main() -> int:
    for path in sorted(VALID.glob("*.json")):
        expected = json.loads(path.read_text(encoding="utf-8"))
        for command in runtimes(path):
            actual = json.loads(run(command, expected_success=True))
            if actual != expected:
                raise RuntimeError(f"semantic round-trip changed {path.name}: {' '.join(command)}")

    for path in sorted(INVALID.glob("*.json")):
        for command in runtimes(path):
            run(command, expected_success=False)

    print(f"Cross-language round-trip: {len(list(VALID.glob('*.json')))} valid and {len(list(INVALID.glob('*.json')))} invalid fixtures passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
