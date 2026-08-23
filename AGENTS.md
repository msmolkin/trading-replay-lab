# AGENTS.md

This file is the authoritative operating guide for all coding agents in this repository. It is deliberately self-contained because cloud agents may not receive user-level instructions.

## Mission

Build a deterministic historical trading game that measures whether a player's decisions survive the real path of a market episode without leaking future information. Correctness, reproducibility, and honest fidelity labels outrank visual polish or simulated realism that the source data cannot support.

## Required reading

Before changing code, read:

1. `README.md`
2. The task you are claiming in `tasks/README.md`
3. The directly relevant document under `docs/`
4. Any ADR referenced by that task

Do **not** read files under `archive/`. They are retained only as historical records and are not requirements or agent instructions.

## Non-negotiable invariants

- Never expose market events later than a session's `revealed_through` timestamp.
- Never fetch historical data directly from the browser. All player-facing data passes through the session visibility gate.
- Never label a fill exact unless its required source fields exist. Fidelity degrades explicitly according to `docs/data-contracts.md`.
- The simulator uses integer fixed-point quantities and prices; no binary floating point enters accounting, matching, fees, margin, or P&L.
- The simulator is a pure deterministic state transition over ordered market events and player commands. Wall-clock time, network time, hash-map iteration order, and unseeded randomness may not affect results.
- Orders, fills, ledger entries, margin changes, and reveals are append-only events. Corrections are compensating events.
- `reduce_only` never crosses through zero or increases absolute exposure. Any excess is canceled with a machine-readable reason.
- `post_only` never executes immediately. It is rejected/canceled if marketable at acceptance, according to the chosen venue profile.
- `marketable_only` never rests. It acts as immediate-or-cancel and cancels unfilled quantity.
- Changing leverage changes margin requirements and liquidation thresholds, not position notional or P&L.
- Raw or licensed market data, API keys, secrets, and provider responses do not enter Git.
- Every imported partition has provenance and content hashes. Timestamps are UTC.

## Repository boundaries

The target monorepo layout is defined in `docs/architecture.md`. Until scaffolding exists, create only the directories assigned by your task.

- `apps/web`: player and review UI; no matching/accounting logic.
- `apps/api`: authentication, setup, visibility gate, session control, queries.
- `crates/sim-core`: authoritative matching, positions, ledger, risk, liquidation.
- `workers/ingest`: provider adapters, normalization, validation, aggregation.
- `packages/contracts`: schemas and generated clients/models.
- `fixtures`: small synthetic or freely redistributable test inputs only.
- `docs/adr`: architectural decision records.

Do not duplicate domain logic across services. If behavior affects money, fills, or visibility, its authority must be named in `docs/architecture.md`.

## How to claim work

Each task in `tasks/README.md` has an ID, dependencies, owned paths, deliverables, and acceptance checks.

1. Choose only a `ready` task whose dependencies are merged.
2. Open or assign an issue named `[TASK-ID] short title`.
3. Comment the paths you intend to touch. One agent owns a path at a time.
4. Keep the change inside those paths. Coordinate contract changes with owners of every consumer.
5. Link the task and include commands/results in the pull request.

Agents may work in parallel only when their owned paths do not overlap and their contract dependencies are already merged. Do not silently edit another agent's in-progress files.

## Engineering workflow

- Make the smallest coherent change that fully satisfies one task.
- Add or update tests in the same change as behavior.
- Format, lint, type-check, and run the narrow test suite before the full affected suite.
- Preserve deterministic fixture outputs. If a golden hash changes, explain the semantic reason in the pull request.
- Prefer generated models from versioned schemas over handwritten cross-language copies.
- Treat warnings about data gaps, sequence gaps, crossed books, stale quotes, and degraded fidelity as product output, not log noise.
- Use UTC ISO 8601 at API boundaries and signed integer epoch nanoseconds internally where supported.
- Include units in names (`price_atoms`, `qty_atoms`, `ts_event_ns`, `fee_minor`).
- Use idempotency keys for commands and ingestion jobs.
- Add an ADR for a cross-cutting, difficult-to-reverse decision. Do not rewrite accepted ADR history; supersede it.

## Testing expectations

At minimum, changes must cover the relevant layers:

- Unit tests for local invariants and boundary values.
- Golden scenario tests for simulator semantics.
- Property tests for accounting conservation, reduce-only behavior, and determinism.
- Contract tests for every adapter and generated model.
- Integration tests proving future events cannot escape through charts, APIs, errors, cache keys, exports, or websocket buffering.
- Replay tests proving identical inputs produce an identical ordered event log and final hash.

See `docs/testing.md` for required scenarios and tolerances. Tests must not call paid or rate-limited providers by default.

## Security and data handling

- Keep credentials in local environment variables or a secrets manager. Commit only `.env.example` with placeholder names.
- Sanitize provider error bodies; they may contain keys, signed URLs, account identifiers, or entitlements.
- Enforce authorization and reveal bounds in server queries, not only UI state.
- Do not log hidden absolute timestamps in blind modes to player-visible logs.
- Never upload provider data to CI artifacts unless redistribution is explicitly permitted.
- Pin fixture licenses and provenance in their manifest.

## Pull request handoff

Every pull request should state:

- Task ID and scope
- Behavioral changes
- Schema or migration impact
- Fidelity/security implications
- Tests run and exact results
- Known limitations and follow-up task IDs

If blocked, leave the repository buildable, document the observed evidence, and identify the smallest decision or dependency needed. Do not substitute a guess for trading/accounting semantics.
