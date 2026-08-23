"""Atomic resumable ingestion checkpoint state."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path


@dataclass(slots=True)
class JobCheckpoint:
    """Completed chunk content hashes for one deterministic job."""

    job_id: str
    completed: dict[str, str] = field(default_factory=dict)


class JobStateStore:
    """Filesystem checkpoint store with atomic replacement."""

    def __init__(self, root: Path) -> None:
        self.root = root

    def _path(self, job_id: str) -> Path:
        return self.root / f"{job_id}.json"

    def load(self, job_id: str) -> JobCheckpoint:
        """Load a checkpoint or return an empty state for a new job."""
        path = self._path(job_id)
        if not path.exists():
            return JobCheckpoint(job_id)
        value: object = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, dict) or value.get("job_id") != job_id:
            raise ValueError("invalid ingestion checkpoint")
        completed_value = value.get("completed")
        if not isinstance(completed_value, dict):
            raise ValueError("invalid ingestion checkpoint chunks")
        completed: dict[str, str] = {}
        for key, digest in completed_value.items():
            if not isinstance(key, str) or not isinstance(digest, str):
                raise ValueError("invalid ingestion checkpoint entry")
            completed[key] = digest
        return JobCheckpoint(job_id, completed)

    def save(self, checkpoint: JobCheckpoint) -> None:
        """Atomically persist a checkpoint in canonical key order."""
        self.root.mkdir(parents=True, exist_ok=True)
        path = self._path(checkpoint.job_id)
        temporary = path.with_suffix(".tmp")
        payload = json.dumps(
            {"completed": checkpoint.completed, "job_id": checkpoint.job_id},
            sort_keys=True,
            separators=(",", ":"),
        )
        temporary.write_text(payload + "\n", encoding="utf-8", newline="\n")
        temporary.replace(path)
