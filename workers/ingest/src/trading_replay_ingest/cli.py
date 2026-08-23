"""Ingestion worker command entry point."""

from __future__ import annotations

import argparse


def message() -> str:
    """Return a side-effect-free readiness message."""
    return "trading-replay-ingest ready"


def main() -> None:
    """Run the lightweight ingestion CLI."""
    parser = argparse.ArgumentParser(prog="trading-replay-ingest")
    parser.add_argument("--version", action="store_true")
    args = parser.parse_args()
    if args.version:
        print("trading-replay-ingest 0.0.0")
    else:
        print(message())
