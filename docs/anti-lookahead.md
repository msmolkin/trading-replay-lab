# Anti-lookahead and sealed episodes

## 1. Threat model

The product prevents accidental and ordinary deliberate discovery of future episode data through its player interfaces. It protects against chart/API inspection, browser developer tools, websocket buffering, cache probing, exports, predictable random selection, verbose errors, and player-visible telemetry.

It cannot make a historical date unknowable to an administrator who controls the server and raw dataset, nor can self-hosted software prevent its owner from editing code or querying files. Results therefore declare a trust profile:

- `LOCAL_PRACTICE`: self-attested, useful for learning, not externally verifiable secrecy.
- `HOSTED_SEALED`: server-enforced visibility with auditable commitments.
- `CHALLENGE_VERIFIED`: hosted sealed run plus pinned build/rules/data proofs and restricted operator access.

## 2. Information policy dimensions

Policies independently control:

- instrument identity: visible, category-only, hidden;
- venue identity: visible or hidden;
- absolute calendar: visible, coarse bucket only, or relative clock;
- setup chart intervals: for example monthly/weekly only;
- active chart intervals: allowed interval set;
- pre-start context length;
- order flow: none, tape, BBO, L2, L3-derived indicators;
- episode selection: explicit, coarse bucket, or random eligible set;
- reveal after completion: automatic, delayed, or challenge-scheduled.

The setup screen must explain what remains inferable. A distinctive price path in a monthly chart can identify a famous episode even if labels are removed.

## 3. Selection without path leakage

### Exact date/time

The player knowingly selects a start. The mode tests execution without future replay, not ignorance of historical context. The session stores the exact selected instant.

### Coarse chart selection

The server constructs setup bars only from permitted information. If the player chooses a weekly/monthly bucket, policy decides whether episode start is the bucket boundary or a committed random instant inside it. Fine bars are never downloaded by the setup client.

### Random eligible selection

Eligibility is computed before sampling using asset/rules/data-quality constraints. The server commits to:

```text
eligible_set_hash = H(canonical eligible episode IDs)
setup_hash        = H(canonical setup + eligible_set_hash)
secret_commitment = H(server_secret + player_nonce + setup_hash)
selection_index   = PRF(server_secret, player_nonce + setup_hash) mod eligible_count
```

Store commitments before revealing any selected data. On completion, reveal inputs needed to reproduce selection. Use a stable canonical encoding and a documented modern hash/PRF; task M3-04 pins the exact algorithms.

## 4. Reveal model

`revealed_through_ns` is monotonic and server-authoritative. Advancing creates an append-only `RevealAdvanced` event before player-visible data is queried. A market event is processable/displayable only if its event time and canonical order are inside the reveal.

For a partially formed bar, aggregate only revealed input. Label it incomplete. Do not fetch a precomputed full bar and hide only its close; high, low, volume, and even response size leak future information.

Order commands observe the last fully applied market state at their recorded arrival sequence. The API cannot accept client-supplied quotes or marks as authoritative.

## 5. Leak surfaces and controls

| Surface | Required control |
|---|---|
| REST/GraphQL | Server-side visibility predicate before scan/aggregation |
| Websocket | Publish persisted revealed events only; no future buffering to client |
| Browser state | Never serialize episode ID/date/future series into page or source maps |
| Cache | Session + visibility generation key; no shared active-session CDN cache |
| Errors | Stable codes; sanitize provider paths, timestamps, counts, and response bodies |
| Logs/metrics | Separate restricted sink; no hidden IDs/timestamps in user-visible diagnostics or metric labels |
| Exports | Active export contains commands/proofs and revealed data only |
| Search/autocomplete | Use precomputed eligible metadata that does not encode selected future path |
| Data-size/timing | Pad or batch highly sensitive challenge responses if inference is material |
| Accessibility | Hidden screen-reader text and chart summaries obey the same policy |
| Support/admin | Audited role access; challenge operators cannot casually inspect assignments |

## 6. Lock semantics

- Setup options become immutable at `SessionCommitted`.
- Lower chart granularity may be unlocked only by a policy-defined reveal progression; a player cannot unlock and relock it in a scored run.
- Changing playback speed never changes visibility entitlements.
- Forking creates a new unscored practice session and records the parent event sequence; it never modifies the original.
- A session does not expose provider-native identifiers if they reveal hidden instrument/date.

## 7. Verification artifacts

Completed hosted runs expose:

- setup, eligible-set, secret, selection, dataset, command-log, domain-event, and result hashes;
- exact simulator, ruleset, score, adapter, and schema versions;
- a proof that the revealed eligible set maps to the committed hash;
- an ordered reveal log;
- a verifier command and expected final hash.

The verifier validates integrity and deterministic execution. It does not prove the operator never saw source data outside the application; that requires operational trust and access controls.

## 8. Required adversarial tests

- Ask every market endpoint for a range extending past reveal.
- Request a coarse candle that straddles reveal.
- Inspect HTML, hydration state, source maps, websocket frames, browser cache, and accessibility tree.
- Force provider errors and malformed symbols.
- Replay stale cache URLs after reveal changes and from another session.
- Race reveal advancement with chart and command requests.
- Disconnect/reconnect and request websocket resumption before/after reveal.
- Export and fork an active session.
- Attempt to infer episode from IDs, row counts, latency, ETags, object keys, and error messages.

Any confirmed future-value disclosure is release-blocking.
