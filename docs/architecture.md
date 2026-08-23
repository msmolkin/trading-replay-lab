# Architecture

## 1. Shape

Trading Replay Lab is an event-sourced modular system with one authoritative deterministic simulator. The first release should remain deployable as a small number of processes; boundaries exist to enable parallel work and correctness, not mandatory microservices.

```text
Provider archives/APIs
        |
        v
  ingest adapters ---> validator ---> canonical Parquet + signed manifest
                                           |
                                           v
Web client <---- session API / visibility gate ----> replay coordinator
                                                        |
                                                        v
                                               deterministic sim-core
                                                        |
                                  append-only session events + snapshots
```

## 2. Monorepo layout

```text
apps/
  web/                 Next.js player, setup, review, accessibility
  api/                 FastAPI control plane and visibility-gated queries
crates/
  sim-core/            Rust command/state/event kernel
  sim-cli/             Offline replay and verification tool
workers/
  ingest/              Python adapters, canonicalization, QA, aggregation
packages/
  contracts/           Source JSON Schemas and generated language models
  ui/                  Shared visual components after web foundations exist
schemas/                Bootstrap contract examples before packages scaffold
fixtures/               Tiny licensed-safe golden scenarios and manifests
docs/                   Normative specifications and ADRs
tasks/                  Parallel execution graph
```

## 3. Component ownership

### Web

Owns presentation, local interaction state, chart rendering, keyboard access, and command intent construction. It is never authoritative for price, fills, account balances, eligibility, or reveal bounds. It must discard websocket messages outside the expected monotonic sequence and resynchronize through the API.

### API/control plane

Owns identity, session lifecycle, setup validation, data entitlements, episode selection, commitments, reveal authorization, command idempotency, export, and read models. It sends canonical commands to sim-core and persists returned events transactionally.

### Replay coordinator

Owns logical time and delivery of ordered canonical market events to sim-core. It prefetches server-side but never places unrevealed values in player-visible stores, payloads, cache keys, logs, metrics labels, or exceptions. It may run inside the API process initially.

### sim-core

Owns order validation that depends on trading state, matching models, order lifecycle, positions, average entry, ledger, fees, funding, margin, liquidation, and deterministic hashes. It performs no network, database, filesystem, or wall-clock I/O. Its interface is a versioned sequence of `InputEnvelope -> Vec<DomainEvent>`.

### Ingestion

Owns provider access, raw caching outside Git, normalization, deduplication, sequence/gap validation, instrument metadata, corporate actions, derived bars, partition manifests, and provider-specific diagnostics. It never implements player fill rules.

### Contract package

Owns canonical JSON Schemas, enums, compatibility checks, OpenAPI bindings, and generated TypeScript/Python/Rust representations. Contract generation is one-way from schemas; generated code is not hand-edited.

## 4. Persistence

- PostgreSQL: users, rulesets, data catalogs, sessions, commitments, command envelopes, domain events, snapshots, challenges, and export metadata.
- Object storage: canonical Parquet partitions, manifests, optional encrypted provider cache, and completed run artifacts.
- DuckDB: local inspection and server-side bounded scans of Parquet during development; not a player-facing database.
- Redis: optional ephemeral job leases and websocket fan-out. Correctness cannot depend on Redis persistence.

Session events use `(session_id, event_seq)` as a strict unique order. Commands have both a user-provided idempotency key and a server command ID. Persist command acceptance and returned domain events in one transaction or recover through deterministic replay.

## 5. Canonical time

- Source timestamps retain both event and receive time when available.
- `ts_event_ns` is the logical market timestamp in UTC.
- `source_sequence` orders venue messages where defined.
- `canonical_tie_breaker` is assigned deterministically during ingestion and documented per adapter.
- `event_seq` is the monotonically increasing order within one canonical stream/session.
- Browser presentation time is derived and may be relative/obscured in blind modes.

No component orders monetary events by local arrival time unless the ruleset explicitly models recorded latency.

## 6. Command/event interface

The simulator processes only declared inputs:

- market event;
- player/system order command;
- cancel/replace command;
- leverage/collateral command;
- scheduled charge/corporate action;
- reveal-independent session control event.

It returns domain events including command accepted/rejected, order state changes, fills, position changes, balanced ledger postings, margin snapshots, warnings, liquidation, and terminal result.

Every envelope includes schema version, simulator/ruleset version, session ID, sequence, logical timestamp, and payload hash. Unknown enum values fail closed.

## 7. Data plane and visibility gate

Canonical partitions may contain the full episode on the server. Player queries require a `SessionVisibility` predicate:

```text
session_id matches authorized principal
AND event.ts_event_ns <= session.revealed_through_ns
AND requested granularity is allowed in current phase
AND fields are allowed by information policy
```

The predicate is applied before aggregation. Otherwise a full future candle could leak its high/low through a request whose start is visible. Cache namespaces include session and reveal generation; public CDN caching is forbidden for active blind-session data.

Websocket producers read the persisted event sequence, not raw prefetched buffers. Exports before completion contain only revealed data and commitments.

## 8. Episode setup and commitment

1. Build an eligible episode list from quality-approved manifests.
2. Canonicalize and hash the setup/ruleset and eligible list.
3. For random modes, derive a draw using server entropy and optionally a player nonce; store a commitment to secret plus setup before selection is revealed.
4. Resolve the immutable episode and dataset partition hashes.
5. Create session with initial reveal bound and store its commitment.
6. On completion, reveal secret, episode identity, and proof inputs so an offline verifier can reproduce selection and hashes.

See `anti-lookahead.md` for threat boundaries.

## 9. Adapter interface

Adapters implement capability discovery, catalog, fetch, normalize, and quality reporting. They output canonical events and never expose provider-native objects downstream.

```text
capabilities() -> DataCapabilities
catalog(query) -> InstrumentCoverage[]
plan(request) -> FetchPlan + cost/entitlement estimate
fetch(plan, checkpoint) -> RawChunk[]
normalize(raw_chunk) -> CanonicalBatch
validate(batch, prior_state) -> QualityReport
```

Checkpoints make jobs restartable. Raw downloads are content-addressed. Canonical partition identity includes adapter version, normalization version, source checksums, instrument definition version, and ordered-event hash.

## 10. Deployment profiles

### Local

Docker Compose starts web, API, PostgreSQL, and an S3-compatible object store. sim-core runs in-process through a stable binding or as sim-cli for verification. Data and credentials remain on the user's machine.

### Hosted

Stateless web/API replicas use managed PostgreSQL/object storage and isolated replay workers. Provider credentials are encrypted per user. Blind-session sensitive traces use restricted observability sinks. Rate, cost, and entitlement limits are enforced before ingestion.

## 11. Failure behavior

- Provider or ingestion failure: job is resumable; no partial partition becomes eligible.
- Sequence gap/corruption: quarantine interval or declare explicit degraded capability; never silently bridge L2/L3 state.
- Replay worker crash: reload latest verified snapshot and replay persisted inputs; output hashes must match.
- Client disconnect: logical clock pauses or continues per pinned session setting; server behavior is authoritative.
- Database write ambiguity: idempotency and expected next sequence prevent duplicate commands/fills.
- Unsupported fidelity/order combination: reject in setup or order validation with actionable reason.

## 12. Performance targets

- Cached F1 single-instrument replay: at least 250,000 market events/second in headless benchmark on a reference development machine.
- Snapshot restore plus integrity check: under 1 second for a normal single-instrument session.
- Player command response: p95 under 100 ms excluding remote ingestion.
- Chart endpoint: p95 under 250 ms for 10,000 already-revealed points.

Correctness tests gate optimization. Parallel replay of independent sessions is allowed; processing inside one session remains ordered.

## 13. Versioning

Pin and export independently:

- contract schema version;
- canonical data version;
- adapter version;
- simulator version;
- ruleset/fill-model version;
- score version;
- web/API build version.

Old completed runs remain verifiable with their pinned versions. Breaking contract changes require migration or a major schema version.
