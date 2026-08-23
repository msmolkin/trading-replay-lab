# Security, privacy, and market-data compliance

## Security baseline

- Browser clients are untrusted; authorization, entitlement, session phase, and reveal bounds are enforced server-side.
- Credentials use a secrets manager in hosted deployments and environment/keychain-backed configuration locally.
- Provider keys are scoped read-only where possible and never sent to sim-core or web.
- All mutating commands require authentication, CSRF protections where applicable, idempotency, expected session version, and audit records.
- Challenge-sensitive administration uses least privilege, audit logs, and time-bounded access.
- Dependencies, containers, and generated artifacts receive supply-chain scanning and pinned lockfiles.
- Export/import validates schemas, sizes, hashes, and decompression limits before use.

## Privacy

Store the minimum identity needed. Trading decisions and performance can be sensitive personal data. Default profiles/runs to private, use opaque IDs, support deletion of user-owned metadata where compatible with challenge integrity, and document retention. Public sharing requires explicit action and redacts provider entitlement/account identifiers.

## Data rights

The project owns its code and original docs, not upstream market facts packaged under contractual feeds. Before enabling a hosted adapter:

1. Record provider and underlying venue terms, effective date, permitted users, display/non-display classification, retention, derived-data, redistribution, caching, and audit obligations.
2. Determine whether each user needs their own license/entitlement.
3. Ensure exports, fixtures, logs, backups, CI, and support tooling follow the same restrictions.
4. Add a kill switch that marks affected manifests `REVOKED` without corrupting completed-run metadata.
5. Obtain legal review where terms are unclear.

Repository documentation is engineering guidance, not legal advice.

## Simulation disclosures

Every setup and result states that fills are hypothetical and counterfactual, historical liquidity may not have remained available after the player's hypothetical action, leverage can cause loss/liquidation, and past simulated results do not predict live performance. Fidelity limitations must be adjacent to results, not buried in terms.

## Out of scope

v1 does not custody assets, connect to brokerage accounts, accept payment for investment advice, or place real orders. Adding any of those requires a separate security/regulatory design and explicit repository-owner approval.
