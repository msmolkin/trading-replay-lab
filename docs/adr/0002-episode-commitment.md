# ADR 0002: Episode commitment and random selection

## Status

Accepted for v1.

## Decision

Random replay selection uses a versioned, domain-separated commitment protocol. The server commits to a 32-byte secret, the selection-affecting setup hash, the canonical eligible-episode hash, and the optional player nonce before returning the selected episode to the control plane.

The eligible list is canonical only when episode identifiers are unique and already in ascending order. The implementation rejects a differently ordered list instead of silently sorting it. Each episode encodes its identifier, manifest SHA-256 digest, and signed 64-bit start/end nanoseconds with fixed-width or length-prefixed fields. Setup fields use the same unambiguous binary encoding. No JSON number or floating-point representation participates in the commitment.

The draw is HMAC-SHA256 keyed by the committed secret over the setup hash, eligible-set hash, player nonce, and an unsigned 64-bit rejection counter. A 256-bit candidate is accepted only below the largest multiple of the candidate count that fits in the 256-bit space. This rejection-sampling rule removes modulo bias. The accepted value modulo the candidate count selects the episode; the counter is included in the completion proof.

The selection secret is encrypted at rest with AES-256-GCM under a server-held 32-byte commitment key and per-record random nonce. Associated data binds the ciphertext to the session and commitment identifiers. The plaintext secret is not stored in `revealed_secret` until the session reaches `COMPLETED`.

The public pre-completion surface contains only algorithm version, commitment hash, setup hash, and eligible-set hash. It does not expose the secret, selected index, draw counter, or selected episode metadata. On completion the server reveals the secret and portable proof inputs. The offline verifier recomputes canonical hashes, commitment, rejection-sampled draw, selected index, and selected episode.

## Consequences

Changing the setup, eligible episode content, canonical episode order, secret, player nonce, draw counter, selected index, or selected episode causes verification to fail. Algorithm changes require a new version string and verifier implementation; v1 proof semantics are immutable.

Commitment correctness does not depend on database row order, process RNG state after the secret/nonce bytes are generated, JSON serialization quirks, native integer endianness, or floating-point arithmetic.
