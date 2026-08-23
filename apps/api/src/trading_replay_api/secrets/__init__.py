"""Principal-scoped provider credential encryption, redaction, and entitlement caching."""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Mapping
from urllib.parse import urlsplit, urlunsplit

from cryptography.hazmat.primitives.ciphers.aead import AESGCM


class CredentialError(RuntimeError):
    """Base credential failure; messages never include secret material."""


class CredentialNotFound(CredentialError):
    """Raised when a principal has no matching credential."""


@dataclass(frozen=True, slots=True)
class SecretValue:
    """Plaintext secret wrapper that is redacted by default string/repr operations."""

    _value: str

    def __str__(self) -> str:
        return "<redacted>"

    def __repr__(self) -> str:
        return "SecretValue(<redacted>)"

    def reveal(self) -> str:
        """Return plaintext only at the explicit provider-call boundary."""
        return self._value


@dataclass(frozen=True, slots=True)
class CredentialMetadata:
    """Non-secret provider credential identity."""

    principal_id: str
    provider: str
    name: str


@dataclass(frozen=True, slots=True)
class _EncryptedCredential:
    metadata: CredentialMetadata
    nonce: bytes
    ciphertext: bytes


class CredentialVault:
    """AES-256-GCM vault keyed and indexed strictly by principal/provider/name."""

    def __init__(self, master_key: bytes) -> None:
        if len(master_key) != 32:
            raise ValueError("credential master key must be exactly 32 bytes")
        self._aead = AESGCM(master_key)
        self._records: dict[tuple[str, str, str], _EncryptedCredential] = {}

    @staticmethod
    def generate_master_key() -> bytes:
        """Generate a fresh AES-256 master key from the operating system CSPRNG."""
        return AESGCM.generate_key(bit_length=256)

    @staticmethod
    def _identity(principal_id: str, provider: str, name: str) -> tuple[str, str, str]:
        identity = (principal_id.strip(), provider.strip().lower(), name.strip())
        if not all(identity):
            raise ValueError("credential identity fields cannot be empty")
        return identity

    @staticmethod
    def _aad(identity: tuple[str, str, str]) -> bytes:
        return "\0".join(identity).encode("utf-8")

    def put(self, principal_id: str, provider: str, name: str, secret: str) -> CredentialMetadata:
        """Encrypt and store a provider credential for exactly one principal."""
        identity = self._identity(principal_id, provider, name)
        if not secret:
            raise ValueError("credential secret cannot be empty")
        nonce = os.urandom(12)
        ciphertext = self._aead.encrypt(nonce, secret.encode("utf-8"), self._aad(identity))
        metadata = CredentialMetadata(*identity)
        self._records[identity] = _EncryptedCredential(metadata, nonce, ciphertext)
        return metadata

    def get(self, principal_id: str, provider: str, name: str) -> SecretValue:
        """Decrypt a credential only through the exact principal-scoped identity."""
        identity = self._identity(principal_id, provider, name)
        record = self._records.get(identity)
        if record is None:
            raise CredentialNotFound("provider credential is unavailable for this principal")
        plaintext = self._aead.decrypt(record.nonce, record.ciphertext, self._aad(identity))
        return SecretValue(plaintext.decode("utf-8"))

    def delete(self, principal_id: str, provider: str, name: str) -> None:
        """Remove only the requesting principal's credential identity."""
        identity = self._identity(principal_id, provider, name)
        if self._records.pop(identity, None) is None:
            raise CredentialNotFound("provider credential is unavailable for this principal")

    def list_metadata(self, principal_id: str) -> tuple[CredentialMetadata, ...]:
        """List non-secret metadata for one principal without exposing other tenants."""
        return tuple(
            sorted(
                (record.metadata for key, record in self._records.items() if key[0] == principal_id),
                key=lambda item: (item.provider, item.name),
            )
        )


@dataclass(frozen=True, slots=True)
class Entitlement:
    """Provider capabilities proven for one principal and feed identity."""

    principal_id: str
    provider: str
    feed: str
    capabilities: frozenset[str]


class EntitlementCache:
    """Tenant-safe cache: principal ID is mandatory in every cache key."""

    def __init__(self) -> None:
        self._values: dict[tuple[str, str, str], Entitlement] = {}

    def put(self, entitlement: Entitlement) -> None:
        key = (entitlement.principal_id, entitlement.provider.lower(), entitlement.feed)
        self._values[key] = entitlement

    def get(self, principal_id: str, provider: str, feed: str) -> Entitlement | None:
        return self._values.get((principal_id, provider.lower(), feed))

    def clear_principal(self, principal_id: str) -> None:
        for key in tuple(self._values):
            if key[0] == principal_id:
                del self._values[key]


_SENSITIVE_HEADERS = {
    "authorization",
    "cookie",
    "proxy-authorization",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
}


def redact_headers(headers: Mapping[str, str]) -> dict[str, str]:
    """Return log-safe headers with known credential-bearing values removed."""
    return {
        key: "<redacted>" if key.lower() in _SENSITIVE_HEADERS else value
        for key, value in headers.items()
    }


def redact_url(url: str) -> str:
    """Remove query/fragment material so signed URLs cannot enter logs or errors."""
    parts = urlsplit(url)
    safe_netloc = parts.hostname or ""
    if parts.port is not None:
        safe_netloc = f"{safe_netloc}:{parts.port}"
    return urlunsplit((parts.scheme, safe_netloc, parts.path, "<redacted>" if parts.query else "", ""))


__all__ = [
    "CredentialError",
    "CredentialMetadata",
    "CredentialNotFound",
    "CredentialVault",
    "Entitlement",
    "EntitlementCache",
    "SecretValue",
    "redact_headers",
    "redact_url",
]
