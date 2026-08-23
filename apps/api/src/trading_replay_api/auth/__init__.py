"""Authentication principals and local/hosted authentication boundary."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Protocol


@dataclass(frozen=True, slots=True)
class Principal:
    """Authenticated security principal."""

    principal_id: str
    display_name: str
    hosted: bool

    def __post_init__(self) -> None:
        if not self.principal_id or not self.display_name:
            raise ValueError("principal identity fields cannot be empty")


class AuthenticationError(PermissionError):
    """Stable authentication failure that never embeds credential material."""


class Authenticator(Protocol):
    """Hosted authentication boundary.

    Transport-specific identity verification stays outside domain code.
    """

    def authenticate(self, headers: Mapping[str, str]) -> Principal:
        """Validate request identity and return a principal."""
        ...


@dataclass(frozen=True, slots=True)
class LocalAuthenticator:
    """Explicit single-user local profile for self-hosted use."""

    principal_id: str = "local-user"
    display_name: str = "Local User"

    def authenticate(self, headers: Mapping[str, str]) -> Principal:
        """Return the configured local principal without consuming remote credentials."""
        del headers
        return Principal(self.principal_id, self.display_name, hosted=False)


@dataclass(frozen=True, slots=True)
class TrustedHeaderAuthenticator:
    """Hosted adapter for identity headers verified by an upstream trusted gateway.

    This class deliberately does not accept bearer tokens. Production deployment must only
    install it behind a gateway that strips untrusted copies of these headers and performs
    cryptographic user authentication itself.
    """

    principal_header: str = "x-trl-principal-id"
    display_header: str = "x-trl-display-name"

    def authenticate(self, headers: Mapping[str, str]) -> Principal:
        principal_id = headers.get(self.principal_header, "").strip()
        display_name = headers.get(self.display_header, "").strip()
        if not principal_id or not display_name:
            raise AuthenticationError("authenticated principal headers are missing")
        if any(character in principal_id for character in "\r\n\x00"):
            raise AuthenticationError("authenticated principal identifier is invalid")
        return Principal(principal_id, display_name, hosted=True)


__all__ = [
    "AuthenticationError",
    "Authenticator",
    "LocalAuthenticator",
    "Principal",
    "TrustedHeaderAuthenticator",
]
