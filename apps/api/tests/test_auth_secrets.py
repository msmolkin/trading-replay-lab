from __future__ import annotations

import pytest

from trading_replay_api.auth import (
    AuthenticationError,
    LocalAuthenticator,
    TrustedHeaderAuthenticator,
)
from trading_replay_api.secrets import (
    CredentialNotFound,
    CredentialVault,
    Entitlement,
    EntitlementCache,
    redact_headers,
    redact_url,
)


def test_local_authentication_is_explicit_single_user_profile() -> None:
    principal = LocalAuthenticator().authenticate({"authorization": "ignored-locally"})
    assert principal.principal_id == "local-user"
    assert principal.hosted is False


def test_hosted_header_authentication_requires_complete_gateway_identity() -> None:
    auth = TrustedHeaderAuthenticator()
    with pytest.raises(AuthenticationError):
        auth.authenticate({})
    with pytest.raises(AuthenticationError):
        auth.authenticate({"x-trl-principal-id": "bad\nvalue", "x-trl-display-name": "Bad"})
    principal = auth.authenticate(
        {"x-trl-principal-id": "user-1", "x-trl-display-name": "Example User"}
    )
    assert principal.principal_id == "user-1"
    assert principal.hosted is True


def test_vault_encrypts_and_isolates_principals() -> None:
    vault = CredentialVault(b"k" * 32)
    vault.put("alice", "Tardis", "api-key", "alice-secret")
    vault.put("bob", "Tardis", "api-key", "bob-secret")

    alice = vault.get("alice", "tardis", "api-key")
    bob = vault.get("bob", "tardis", "api-key")
    assert alice.reveal() == "alice-secret"
    assert bob.reveal() == "bob-secret"
    assert str(alice) == "<redacted>"
    assert repr(alice) == "SecretValue(<redacted>)"
    assert "alice-secret" not in repr(vault.list_metadata("alice"))

    with pytest.raises(CredentialNotFound) as missing:
        vault.get("mallory", "tardis", "api-key")
    assert "alice-secret" not in str(missing.value)
    assert "bob-secret" not in str(missing.value)


def test_vault_delete_cannot_cross_principal_boundary() -> None:
    vault = CredentialVault(b"m" * 32)
    vault.put("alice", "provider", "token", "secret")
    with pytest.raises(CredentialNotFound):
        vault.delete("bob", "provider", "token")
    assert vault.get("alice", "provider", "token").reveal() == "secret"


def test_entitlement_cache_never_falls_back_across_principals() -> None:
    cache = EntitlementCache()
    cache.put(Entitlement("alice", "databento", "GLBX.MDP3", frozenset({"MBO"})))
    assert cache.get("alice", "DATABENTO", "GLBX.MDP3") is not None
    assert cache.get("bob", "DATABENTO", "GLBX.MDP3") is None
    cache.clear_principal("bob")
    assert cache.get("alice", "databento", "GLBX.MDP3") is not None
    cache.clear_principal("alice")
    assert cache.get("alice", "databento", "GLBX.MDP3") is None


def test_log_redaction_removes_credentials_and_signed_url_material() -> None:
    headers = redact_headers(
        {
            "Authorization": "Bearer ultra-secret",
            "X-Api-Key": "provider-secret",
            "Accept": "application/json",
        }
    )
    assert headers["Authorization"] == "<redacted>"
    assert headers["X-Api-Key"] == "<redacted>"
    assert headers["Accept"] == "application/json"

    safe = redact_url(
        "https://user:password@example.test/data/file?X-Amz-Signature=super-secret&token=bad#frag"
    )
    assert safe == "https://example.test/data/file?<redacted>"
    assert "password" not in safe
    assert "super-secret" not in safe
    assert "token=bad" not in safe
