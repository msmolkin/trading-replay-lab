"""Content-addressed raw cache with read-time integrity verification."""

from __future__ import annotations

import hashlib
from pathlib import Path


class CacheCorruption(RuntimeError):
    """Raised when cached bytes no longer match their content address."""


class ContentAddressedCache:
    """Simple SHA-256 file cache kept outside committed repository data."""

    def __init__(self, root: Path) -> None:
        self.root = root

    def _path(self, digest: str) -> Path:
        return self.root / digest[:2] / digest[2:]

    def put(self, payload: bytes) -> str:
        """Persist bytes once and return their SHA-256 content address."""
        digest = hashlib.sha256(payload).hexdigest()
        path = self._path(digest)
        if path.exists():
            if self.get(digest) != payload:
                raise CacheCorruption(digest)
            return digest
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_suffix(".tmp")
        temporary.write_bytes(payload)
        temporary.replace(path)
        return digest

    def get(self, digest: str) -> bytes:
        """Load and verify a content-addressed object."""
        payload = self._path(digest).read_bytes()
        if hashlib.sha256(payload).hexdigest() != digest:
            raise CacheCorruption(digest)
        return payload
