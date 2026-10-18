# ADR-0002: Go simulation core

- Status: accepted
- Date: 2026-10-17
- Supersedes: ADR-0001

## Context

The repository began with a Rust simulator, but the project-wide implementation policy now selects Go for new projects unless Rust-specific memory or concurrency guarantees are decisive. This simulator does not require unsafe-memory control, a no-GC runtime, or compile-time data-race prevention.

## Decision

New authoritative simulator and verifier work is written in Go under `internal/` and `cmd/`. The web UI sends commands through a server boundary and never reimplements accounting. Fixed-point integers, deterministic ordering, no wall-clock or network I/O in the core, and player-safe snapshots remain mandatory.

The existing Rust crates remain read-only migration reference until Go conformance covers their accepted scenarios. They are not the target for new features.

## Consequences

The first playable practice slice now runs through a Go core and HTTP adapter. A separate contract task must add generated Go models before the full canonical simulator consumes schema bindings. Rust removal is deferred until parity is demonstrated; deleting it earlier would discard working conformance evidence.

## Verification

Go unit, HTTP integration, race, and vet checks cover no-lookahead snapshots, exact integer accounting, order semantics, leverage, margin, completion, and liquidation. Future migration tasks must replay canonical fixtures against both implementations until Go becomes the sole verified core.
