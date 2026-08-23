# Parallel implementation graph

This is the claimable work queue. Each task is designed for one focused pull request and names exclusive paths. Dependencies must be merged, not merely open. Update task status in the same pull request that satisfies its dependency.

Status meanings: `ready` can be claimed now; `blocked` is waiting on listed tasks; `done` has merged acceptance evidence; `active` has a linked assignee/issue.

## Critical path

```text
M0-01 bootstrap
  ├─ M0-02 contracts ─┬─ simulator stream ─ risk ─ conformance
  │                   ├─ API/session ─ visibility ─ integration
  │                   ├─ ingestion base ─ free crypto adapter ─ catalog
  │                   └─ web base ─ setup/chart/ticket ─ game shell
  └─ M0-03 CI/tooling supports every branch
```

After M0-01 and M0-02, most simulator, ingestion, API, and web work can proceed in parallel. Contract changes are serialized through M0-02's generated-schema process.

## Milestone 0 — Repository foundation

### M0-01 — Monorepo bootstrap (`ready`)

- Depends: none
- Owns: root build manifests/lockfiles, `apps/`, `crates/`, `workers/`, `packages/` directory scaffolds only
- Deliver: pinned Rust/Python/Node toolchains; workspace commands for format/lint/type/test; minimal hello-world packages; `.env.example`.
- Accept: clean clone executes one documented bootstrap command and all empty-stack checks; no domain behavior is introduced.

### M0-02 — Contract source and code generation (`blocked`: M0-01)

- Owns: `packages/contracts/**`, `schemas/**`
- Deliver: JSON Schema source layout; generated TypeScript/Python/Rust models; schema version check; order, market event, manifest, command/event, visibility, and result bundle types.
- Accept: valid examples round-trip in all three languages; incompatible major and unsafe JSON integers fail.

### M0-03 — CI, policy, and repository checks (`blocked`: M0-01)

- Owns: `.github/workflows/**`, root formatter/linter configs, secret/large-file policy configs
- Deliver: pull-request and nightly matrices described in `docs/testing.md`; dependency caching; generated-code drift and schema compatibility checks.
- Accept: intentionally malformed formatting, generated drift, secret fixture, and Parquet file are rejected in a test branch.

### M0-04 — Synthetic fixture generator (`blocked`: M0-02)

- Owns: `fixtures/**`, `tools/fixture-generator/**`
- Deliver: deterministic generator and fixture license/provenance manifests for F0/F1/F2 micro-markets.
- Accept: regeneration is byte-identical and covers timestamp ties, gaps, partial depth, funding, and corporate-action examples.

## Milestone 1 — Deterministic simulator

### M1-01 — Fixed-point primitives and instruments (`blocked`: M0-02)

- Owns: `crates/sim-core/src/numeric/**`, `crates/sim-core/src/instrument/**`
- Deliver: checked integer price/qty/money/rate types, rounding policies, linear contract math, point-in-time instrument validation.
- Accept: boundary/overflow tests and differential arbitrary-precision tests pass; no float enters public domain types.

### M1-02 — Event kernel and hash chain (`blocked`: M0-02)

- Owns: `crates/sim-core/src/kernel/**`, `crates/sim-core/src/hash/**`
- Deliver: pure input transition shell, canonical serialization, ordered output, prior/current hashes, snapshot state-version hooks.
- Accept: repeated runs and input repartitioning yield identical output; invalid sequence/version fails closed.

### M1-03 — Order state machine (`blocked`: M1-01, M1-02)

- Owns: `crates/sim-core/src/orders/**`
- Deliver: validation/lifecycle, cancel/replace, GTC/IOC/FOK, reduce-only live cap, post-only, marketable-only, stop trigger conversion.
- Accept: all order-state golden scenarios and generated legal-transition tests pass.

### M1-04 — Positions and balanced ledger (`blocked`: M1-01, M1-02)

- Owns: `crates/sim-core/src/positions/**`, `crates/sim-core/src/ledger/**`
- Deliver: average entry, realized P&L, partial close, zero crossing into two accounting legs, balanced postings, economic snapshots.
- Accept: crossing scenarios reconcile exactly; every transaction balances and reversal basis is correct.

### M1-05 — F0 bar execution (`blocked`: M1-03, M1-04, M0-04)

- Owns: `crates/sim-core/src/execution/f0/**`
- Deliver: next-open markets, limit/stop reach, pessimistic/optimistic/seeded intrabar policy, uncertainty flags, configurable slippage.
- Accept: no order fills on a prior bar; ambiguous stop/liquidation scenario is pinned and flagged.

### M1-06 — F1 trade/BBO execution (`blocked`: M1-03, M1-04, M0-04)

- Owns: `crates/sim-core/src/execution/f1/**`
- Deliver: quote-based taker model, later-print resting fill eligibility, queue heuristic interface, staleness and size caps.
- Accept: bid/ask/midpoint, maker approximation, partial IOC, and gap fixtures pass without using future prints.

### M1-07 — F2 depth and queue model (`blocked`: M1-03, M1-04, M0-04)

- Owns: `crates/sim-core/src/execution/f2/**`
- Deliver: L2 book state, snapshot/delta recovery, multi-level sweeps, displayed queue-ahead and cancel allocation policies.
- Accept: depth conservation, sequence-gap disablement, sweep fills, and queue fixtures pass.

### M1-08 — F3 MBO queue model (`blocked`: M1-03, M1-04, M0-04)

- Owns: `crates/sim-core/src/execution/f3/**`
- Deliver: visible order lifecycle and counterfactual player queue insertion with explicit size/impact caps.
- Accept: add/modify/cancel/fill/clear and reconnect fixtures reconstruct expected book; uncertainty remains reported.

### M1-09 — Fees and scheduled economics (`blocked`: M1-01, M1-04)

- Owns: `crates/sim-core/src/economics/**`
- Deliver: maker/taker fees/rebates, funding, borrow, dividends, split adjustments, futures settlement hooks.
- Accept: charges post exactly once with declared rounding; split preserves value and adjusts working orders.

### M1-10 — Margin, leverage, and liquidation (`blocked`: M1-03, M1-04, M1-09)

- Owns: `crates/sim-core/src/risk/**`
- Deliver: synthetic isolated initial/maintenance margin, working-order reservation, 1×–50× changes, margin precheck, liquidation state machine.
- Accept: leverage never scales P&L, invalid decrease is atomic, reversal margin partial is correct, funding can deterministically liquidate.

### M1-11 — Simulator facade and snapshots (`blocked`: M1-05, M1-06, M1-07, M1-09, M1-10)

- Owns: `crates/sim-core/src/lib.rs`, `crates/sim-core/src/facade/**`, `crates/sim-core/src/snapshot/**`
- Deliver: public versioned `InputEnvelope -> DomainEvent[]`, execution-model registry, periodic state snapshots and restore.
- Accept: uninterrupted/restored runs match hashes; unsupported tier/order combinations fail with stable codes.

### M1-12 — Offline verifier CLI (`blocked`: M1-11, M0-02)

- Owns: `crates/sim-cli/**`
- Deliver: replay canonical inputs, inspect ledger, verify proof bundle/hash chain, emit machine-readable result.
- Accept: valid bundle reproduces result; tampered command/event/manifest hash identifies exact failure.

### M1-13 — Simulator conformance/property suite (`blocked`: M1-11, M0-04)

- Owns: `crates/sim-core/tests/**`, `fixtures/scenarios/**` expected simulator outputs
- Deliver: all scenarios in `docs/testing.md`, property strategies, platform determinism tests, mutation targets for critical guards.
- Accept: all normative invariants run in CI and seeded failures are reproducible.

## Milestone 2 — Ingestion and catalog

### M2-01 — Ingestion framework (`blocked`: M0-02)

- Owns: `workers/ingest/src/core/**`, `workers/ingest/src/cli.py`
- Deliver: adapter protocol, fetch plans/checkpoints, content-address cache, rate/cost guard, canonical writer, idempotent job state.
- Accept: fake adapter resumes interrupted chunks and creates identical output/manifest.

### M2-02 — Manifest and quality validator (`blocked`: M2-01, M0-04)

- Owns: `workers/ingest/src/quality/**`
- Deliver: counts, timestamps, duplicates, increments, gaps, book continuity, crossed/stale intervals, immutable status decision.
- Accept: corrupt/gapped fixture quarantines or downgrades exactly as declared; clean rerun hash is identical.

### M2-03 — Canonical file adapter (`blocked`: M2-01)

- Owns: `workers/ingest/src/adapters/canonical_file/**`
- Deliver: documented CSV/Parquet import mapping with explicit capability/provenance declarations and safe resource limits.
- Accept: valid sample imports; path traversal, zip bomb, float money, and undeclared columns fail closed.

### M2-04 — Bar aggregation and calendars (`blocked`: M2-01)

- Owns: `workers/ingest/src/aggregate/**`, `workers/ingest/src/calendars/**`
- Deliver: deterministic trade-to-bar aggregation, UTC crypto and session-aware equity/futures intervals, incomplete/gap flags.
- Accept: multi-interval reconciliation passes across DST, week/month, halt, and empty-period fixtures.

### M2-05 — Binance Public Data adapter (`blocked`: M2-01, M2-02)

- Owns: `workers/ingest/src/adapters/binance_public/**`, `workers/ingest/docs/binance_public/**`
- Deliver: spot/USD-M/COIN-M catalog and checksum downloads for klines/trades/aggregate trades; date/product timestamp-unit handling; archive revision detection.
- Accept: opt-in official smoke plus recorded schema fixtures; output declares F0/F0T and never BBO/depth.

### M2-06 — Tardis adapter (`blocked`: M2-01, M2-02)

- Owns: `workers/ingest/src/adapters/tardis/**`, `workers/ingest/docs/tardis/**`
- Deliver: entitlement/cost plan, trades/BBO/L2/funding/liquidations, snapshot/delta reconstruction inputs and coverage discovery.
- Accept: sample-day smoke and gap/reconnect fixtures; capability is computed per venue/instrument/interval.

### M2-07 — Databento adapter (`blocked`: M2-01, M2-02)

- Owns: `workers/ingest/src/adapters/databento/**`, `workers/ingest/docs/databento/**`
- Deliver: definitions, OHLCV/trades/BBO/MBP/MBO/status/statistics/corporate actions as entitled; batch cost confirmation; futures individual-contract symbology.
- Accept: metered calls require explicit budget; official sample/small-credit smoke and point-in-time definition fixture pass.

### M2-08 — Alpaca adapter (`blocked`: M2-01, M2-02)

- Owns: `workers/ingest/src/adapters/alpaca/**`, `workers/ingest/docs/alpaca/**`
- Deliver: stocks/crypto bars/trades/quotes, feed/plan provenance, pagination, adjustments and rate limits.
- Accept: free/sandbox smoke where available; IEX/SIP-like feed identity cannot be conflated.

### M2-09 — Coverage catalog service (`blocked`: M2-02, M2-03)

- Owns: `apps/api/src/catalog/**`, catalog database migrations
- Deliver: eligible coverage segments, warm-up/duration intersection, capability/fidelity queries, manifest revoke/version behavior.
- Accept: gaps split intervals; exact setup eligibility is reproducible from manifest set and ruleset.

## Milestone 3 — API, persistence, and sealed sessions

### M3-01 — Database/event store (`blocked`: M0-02, M0-01)

- Owns: `apps/api/src/db/**`, non-catalog migrations
- Deliver: session/command/domain-event/snapshot/ruleset/commitment models; transactions, optimistic versioning, hash-chain constraints.
- Accept: duplicate/racing commands cannot duplicate events; migration up/down policy is documented and tested.

### M3-02 — Session setup and lifecycle (`blocked`: M3-01, M2-09)

- Owns: `apps/api/src/sessions/**`
- Deliver: setup validation, commit/start/pause/advance/complete/fork state machine and stable error codes.
- Accept: illegal transitions fail; committed rules/data hashes are immutable; unsupported fidelity combinations reject.

### M3-03 — Replay coordinator (`blocked`: M3-01, M1-11)

- Owns: `apps/api/src/replay/**`
- Deliver: ordered partition cursor, logical clock, command insertion, snapshot recovery, websocket persisted-event publisher.
- Accept: crash/resume equals uninterrupted replay; speed/batching never changes canonical result.

### M3-04 — Episode commitments and random selection (`blocked`: M3-02)

- Owns: `apps/api/src/commitments/**`, `docs/adr/0002-episode-commitment.md`
- Deliver: canonical eligible-set/setup encoding, secure selection algorithm, pre-selection commitments, completion proof.
- Accept: verifier reproduces selection; changed order/setup/secret fails; no modulo/encoding ambiguity.

### M3-05 — Visibility-gated market API (`blocked`: M3-02, M3-03, M2-04)

- Owns: `apps/api/src/market/**`, `apps/api/src/visibility/**`
- Deliver: bars/tape/BBO/depth endpoints and websocket frontier applying visibility before aggregation; session-scoped caching.
- Accept: the complete adversarial canary suite finds no future bytes; straddling candles are partial.

### M3-06 — Trading command API (`blocked`: M3-02, M3-03)

- Owns: `apps/api/src/commands/**`
- Deliver: order/cancel/replace/leverage endpoints, idempotency, expected-version handling, quote shortcut resolution, command result queries.
- Accept: retries are stable; shortcuts cite visible quote; client cannot inject authoritative market/ledger fields.

### M3-07 — Authentication and provider secrets (`blocked`: M3-01)

- Owns: `apps/api/src/auth/**`, `apps/api/src/secrets/**`
- Deliver: local single-user profile, hosted auth interface, per-principal provider credential encryption/redaction and entitlement boundary.
- Accept: cross-principal reads/caches fail; error/log scans reveal no credentials or signed URLs.

### M3-08 — Results, scoring, and export (`blocked`: M3-02, M3-03, M3-04, M1-12)

- Owns: `apps/api/src/results/**`, `apps/api/src/exports/**`
- Deliver: versioned metrics/lexicographic score, benchmark, completion reveal, portable proof bundle.
- Accept: exported run verifies offline; active export excludes future data; fee/funding/drawdown metrics reconcile.

## Milestone 4 — Web application

### M4-01 — UI foundations and design system (`blocked`: M0-01, M0-02)

- Owns: `apps/web/src/components/foundation/**`, `apps/web/src/styles/**`, `packages/ui/**`
- Deliver: theme/tokens, responsive shell, accessible controls/dialogs/tables, formatting for fixed-point values and hidden dates.
- Accept: keyboard and screen-reader smoke; no unsafe bigint-to-number conversion.

### M4-02 — Setup and preflight (`blocked`: M4-01, M3-02)

- Owns: `apps/web/src/features/setup/**`
- Deliver: mode/universe/date/duration/equity/leverage/rules/information controls, coarse chart selector, fidelity/cost/gap preflight, commit confirmation.
- Accept: only eligible combinations submit; 1×–50× and authentic caps are clear; setup locks after commit.

### M4-03 — Visibility-safe chart and playback (`blocked`: M4-01, M3-05)

- Owns: `apps/web/src/features/chart/**`, `apps/web/src/features/playback/**`
- Deliver: permitted intervals, incomplete candle, relative/hidden time, playback/step/pause, reconnect/resync, fidelity/gap indicators.
- Accept: browser state/source/accessibility scan has no canary; disconnect resumes monotonic sequence.

### M4-04 — Order ticket (`blocked`: M4-01, M3-06)

- Owns: `apps/web/src/features/order-ticket/**`
- Deliver: buy/sell/short/cover/reverse/close presets; quantity/target; market/limit/stop; bid/ask/midpoint; reduce/post/marketable-only and TIF; validation/reject feedback.
- Accept: every intent maps to the documented canonical command; incompatible flags cannot submit; crossing preview is explicit.

### M4-05 — Account, position, orders, and order flow (`blocked`: M4-01, M3-03, M3-05)

- Owns: `apps/web/src/features/account/**`, `apps/web/src/features/orders/**`, `apps/web/src/features/order-flow/**`
- Deliver: equity/P&L/margin/liquidation buffer, working orders/audit tape, optional BBO/depth/trade tape driven by capabilities.
- Accept: event-sequence updates reconcile after resync; unsupported order-flow views are absent, not fabricated.

### M4-06 — Active game shell (`blocked`: M4-02, M4-03, M4-04, M4-05)

- Owns: `apps/web/src/app/session/**`
- Deliver: responsive composition, loading/error/terminal states, command shortcuts with confirmation safety, persistent fidelity warning.
- Accept: complete fixture episode is playable at desktop/tablet sizes with keyboard only.

### M4-07 — Review and proof UI (`blocked`: M4-01, M3-08)

- Owns: `apps/web/src/features/review/**`
- Deliver: full-path decision timeline, metrics/benchmark/cost attribution, fidelity uncertainty, proof/hash/version and export controls.
- Accept: values match result contract; liquidated and ambiguous-bar reports explain outcome without overstating precision.

### M4-08 — Accessibility and leak audit (`blocked`: M4-06, M4-07)

- Owns: `apps/web/tests/accessibility/**`, `apps/web/tests/leak-audit/**`
- Deliver: automated/manual WCAG-oriented flows and full browser future-canary inspection.
- Accept: no serious automated violations in core flows and no hidden future canary in any inspected surface.

## Milestone 5 — Integrated release

### M5-01 — Local Docker profile (`blocked`: M0-01, M3-03, M4-06, M2-05)

- Owns: `deploy/local/**`, root `compose.yaml`
- Deliver: web/API/Postgres/object store/worker local stack, volumes, health checks, free Binance import walkthrough and synthetic instant-start path.
- Accept: clean machine reaches a playable synthetic episode and documented free-data import without secrets in images/logs.

### M5-02 — End-to-end scenario suite (`blocked`: M5-01, M4-07)

- Owns: `tests/e2e/**`
- Deliver: browser/API flows for every session mode, zero crossing, flags, leverage, stops, reconnect, liquidation, export/verify.
- Accept: deterministic expected hashes and canary leak suite pass in CI.

### M5-03 — Performance harness (`blocked`: M1-11, M3-03, M3-05)

- Owns: `benchmarks/**`
- Deliver: simulator throughput, restore, chart query, command latency and websocket measurements with fixed datasets/reference metadata.
- Accept: targets in `docs/architecture.md` are measured; regressions have enforced budgets after baseline approval.

### M5-04 — Observability and runbooks (`blocked`: M3-03, M3-07)

- Owns: `ops/observability/**`, `docs/runbooks/**`
- Deliver: health/SLO metrics, privacy-safe tracing/logging, ingestion/replay/data-revoke/credential-incident runbooks.
- Accept: injected provider gap, replay crash, database ambiguity, and secret-like error are detected/recovered without player-visible leak.

### M5-05 — Security and data-rights review (`blocked`: M5-02, M5-04)

- Owns: `security/**`, adapter `LEGAL.md` review updates
- Deliver: threat model, entitlement tests, dependency/container scan response, provider redistribution checklist, release risk register.
- Accept: no open critical/high issue; future-data disclosure blocks release; every enabled adapter has an approved rights record.

### M5-06 — v0.1 documentation and release (`blocked`: M5-02, M5-03, M5-05)

- Owns: user-facing additions to `README.md`, `docs/getting-started/**`, release workflow/changelog
- Deliver: local install, free crypto import, gameplay, fidelity interpretation, verifier, troubleshooting, signed/tagged release evidence.
- Accept: a new contributor follows docs from clean clone to completed verified run; limitations and simulation disclosure are prominent.

## Cross-cutting change protocol

Some changes necessarily touch owned boundaries. Open a small contract/ADR task first, list all consumers, merge schema/domain decision, regenerate models, then allow consumer tasks to proceed. Do not combine a speculative contract redesign with a UI or adapter feature.

If a task is too large for one reviewable pull request, split it by adding child IDs (for example `M2-07a`) with non-overlapping paths and explicit merge order. Preserve the acceptance criteria of the parent.
