#!/usr/bin/env python3
"""Fail closed on repository data, secret, and oversized-file policy violations."""

from __future__ import annotations

import argparse
import re
import subprocess
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Policy:
    max_tracked_file_bytes: int
    forbidden_extensions: frozenset[str]
    text_scan_limit_bytes: int
    allowed_secret_assignment_values: frozenset[str]


TOKEN_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("private key material", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")),
    ("AWS access key", re.compile(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b")),
    ("GitHub token", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})\b")),
    ("OpenAI-style secret", re.compile(r"\bsk-[A-Za-z0-9_-]{20,}\b")),
)
SECRET_NAME_RE = re.compile(
    r"(?i)^(?:[A-Za-z0-9_.-]*)(?:API_KEY|API_SECRET|PASSWORD|TOKEN|SECRET_KEY)$"
)


def load_policy(root: Path) -> Policy:
    data = tomllib.loads((root / "repo-policy.toml").read_text(encoding="utf-8"))
    return Policy(
        max_tracked_file_bytes=int(data["max_tracked_file_bytes"]),
        forbidden_extensions=frozenset(str(value) for value in data["forbidden_extensions"]),
        text_scan_limit_bytes=int(data["text_scan_limit_bytes"]),
        allowed_secret_assignment_values=frozenset(
            str(value).lower() for value in data["allowed_secret_assignment_values"]
        ),
    )


def tracked_paths(root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    return [root / raw.decode("utf-8") for raw in result.stdout.split(b"\0") if raw]


def secret_assignment(line: str) -> tuple[str, str] | None:
    stripped = line.strip()
    if not stripped or stripped.startswith("#"):
        return None
    separators = [index for index in (stripped.find("="), stripped.find(":")) if index >= 0]
    if not separators:
        return None
    split_at = min(separators)
    name = stripped[:split_at].strip()
    if not SECRET_NAME_RE.fullmatch(name):
        return None
    value = stripped[split_at + 1 :].strip()
    if value[:1] in {"\"", "'"} and value[-1:] == value[:1]:
        value = value[1:-1]
    value = value.split("#", 1)[0].strip()
    return name, value


def inspect_path(path: Path, root: Path, policy: Policy) -> list[str]:
    rel = path.relative_to(root).as_posix()
    if not path.is_file():
        return []

    violations: list[str] = []
    size = path.stat().st_size
    if size > policy.max_tracked_file_bytes:
        violations.append(
            f"{rel}: tracked file is {size} bytes; limit is {policy.max_tracked_file_bytes}"
        )
    if path.suffix.lower() in policy.forbidden_extensions:
        violations.append(f"{rel}: forbidden tracked data extension {path.suffix.lower()}")
    if size > policy.text_scan_limit_bytes:
        return violations

    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return violations

    for label, pattern in TOKEN_PATTERNS:
        if pattern.search(text):
            violations.append(f"{rel}: possible {label}")

    for line in text.splitlines():
        assignment = secret_assignment(line)
        if assignment is None:
            continue
        name, raw_value = assignment
        value = raw_value.lower()
        if value.startswith("${") or value in policy.allowed_secret_assignment_values:
            continue
        violations.append(f"{rel}: possible populated secret assignment for {name}")
        break

    return violations


def scan(root: Path, paths: list[Path] | None = None) -> list[str]:
    policy = load_policy(root)
    candidates = tracked_paths(root) if paths is None else paths
    violations: list[str] = []
    for path in candidates:
        violations.extend(inspect_path(path, root, policy))
    return violations


def self_test(root: Path) -> None:
    policy = load_policy(root)
    with tempfile.TemporaryDirectory(dir=root) as temp_dir:
        base = Path(temp_dir)
        secret = base / "secret.txt"
        secret.write_text("ALPACA_API_SECRET=definitely-not-a-placeholder\n", encoding="utf-8")
        parquet = base / "market.parquet"
        parquet.write_bytes(b"PAR1")
        benign = base / "safe.env"
        benign.write_text(
            "ALPACA_API_SECRET=replace-me\nBINANCE_API_KEY=\nTOKEN=${RUNTIME_TOKEN}\n",
            encoding="utf-8",
        )

        assert inspect_path(secret, root, policy), "secret fixture must be rejected"
        assert inspect_path(parquet, root, policy), "Parquet fixture must be rejected"
        assert not inspect_path(benign, root, policy), "placeholder/blank fixture must be accepted"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()

    if args.self_test:
        self_test(root)

    violations = scan(root)
    if violations:
        print("Repository policy violations:")
        for violation in violations:
            print(f"- {violation}")
        return 1
    print("Repository policy: OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
