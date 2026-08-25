# Offline replay verifier

`sim-cli` verifies a portable proof package without provider credentials or network access.

```text
sim-cli verify proof-bundle.json
sim-cli ledger proof-bundle.json
```

`verify` writes one machine-readable JSON object and exits `0` only when every commitment and invariant matches. Invalid proofs exit `1`; usage or file I/O errors exit `2`. `ledger` independently verifies balanced transactions and prints exact per-account/currency balances.

## Why the proof package contains canonical inputs

The public `schemas/v1/result-bundle.schema.json` records commands, domain events, state hashes, and manifest hashes, but it does not contain the ordered canonical market/economic input bytes needed to reproduce the simulator kernel hash chain. M3-08 must therefore export those canonical inputs alongside the public result fields. This verifier defines that reproducible section instead of pretending manifest hashes alone contain the underlying market data.

## Proof JSON v1

All authoritative integer fields are canonical decimal **strings**. Floating-point JSON numeric tokens are rejected by the verifier. Hashes are lowercase SHA-256 hexadecimal. Payloads are lowercase hexadecimal bytes.

Required top-level fields:

- `verification_version`: currently `"1"`.
- `session_id`: simulator session identifier.
- `manifest_hashes`: manifest content hashes used by the run.
- `manifest_set_hash`: `TRL-MANIFEST-SET-v1` commitment over the sorted manifest hashes.
- `inputs`: ordered canonical kernel inputs with `session_id`, `input_seq`, `expected_state_version`, `logical_ts_ns`, `kind`, and `payload_hex`.
- `kernel_events`: expected kernel events with `event_seq`, `state_version`, `logical_ts_ns`, `kind`, `payload_hash`, `prior_event_hash`, and `current_event_hash`.
- `state_hashes`: ordered simulator state commitments as `{event_seq, hash}`.
- `state_hashes_hash`: `TRL-STATE-HASHES-v1` commitment over that list.
- `ledger_transactions`: ordered transactions with `event_seq`, `transaction_id`, and at least two `{account, amount_minor, currency}` postings. Every currency inside each transaction must sum to zero independently.
- `ledger_hash`: `TRL-LEDGER-PROOF-v1` commitment. Posting order is canonicalized before hashing.
- `metrics`: `survived`, `terminal_return_ppb`, `max_drawdown_ppb`, `peak_effective_leverage_ppb`, and `benchmark_return_ppb`.
- `result_hash`: `TRL-VERIFIER-RESULT-v1` commitment over session identity, manifest commitment, reproduced final kernel hash, state-hash commitment, ledger commitment, and metrics.

The verifier replays `inputs` through the same `sim_core::kernel::Kernel` used by the simulator. Each reproduced event must exactly match its corresponding `kernel_events` entry. This detects payload/command changes, sequence/version tampering, changed event hashes, or changed causal ordering at the exact input index.

## Failure codes

- `FORMAT`: malformed or non-canonical proof encoding.
- `MANIFEST_HASH`: malformed manifest hash and exact array index.
- `MANIFEST_COMMITMENT`: manifest list differs from its commitment.
- `INPUT_SEQUENCE`: session, input sequence, or expected state-version mismatch.
- `EVENT_MISMATCH`: reproduced kernel event differs from the declared event at the exact index.
- `STATE_HASH`: invalid state-hash ordering.
- `STATE_COMMITMENT`: state hash list differs from its commitment.
- `LEDGER_FORMAT`: invalid transaction structure/order/identity.
- `LEDGER_IMBALANCE`: transaction does not balance exactly, with transaction index.
- `LEDGER_COMMITMENT`: ledger differs from its commitment.
- `RESULT_COMMITMENT`: final result commitment does not reproduce.

No proof verification path uses floating-point arithmetic.
