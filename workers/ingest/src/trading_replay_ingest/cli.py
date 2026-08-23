"""Minimal ingestion command used to prove the workspace scaffold."""


def message() -> str:
    return "trading-replay-ingest bootstrap ready"


def main() -> None:
    print(message())
