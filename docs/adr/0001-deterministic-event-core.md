# ADR-0001: Deterministic event-sourced simulation core

- Status: accepted
- Date: 2026-08-23
- Owners: repository maintainers

## Context

The game must reproduce path-dependent fills, accounting, margin, and liquidation across local, hosted, and verifier environments. Multiple agents and languages will implement surrounding components.

## Decision

Use one Rust `sim-core` as the monetary/trading authority. It accepts versioned ordered input envelopes and returns append-only domain events without I/O or wall-clock access. All canonical numeric values are fixed-point integers. Hashes use a pinned canonical serialization. API and UI consume generated contracts and do not reimplement domain behavior.

## Consequences

Bindings and cross-language schema generation add setup work. In return, one deterministic boundary supports fast replay, golden fixtures, offline verification, and independent UI/API/ingestion work.

## Alternatives considered

- Python-only core: fast iteration but weaker control over numeric/runtime determinism and throughput.
- TypeScript-only core: convenient UI sharing but risks duplicating trust boundaries and server/browser semantics.
- Per-service trading logic: rejected because drift would make results unverifiable.

## Verification

Golden replay hashes match across supported platforms and snapshot restoration. Property tests prove accounting and order invariants. API integration uses only the declared command/event interface.
