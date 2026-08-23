//! Pure deterministic input sequencing, state-versioning, and event hash chaining.

use core::fmt;

use crate::hash::{CanonicalWriter, Hash32, ZERO_HASH, sha256};

/// Versioned authoritative input accepted by the deterministic kernel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputEnvelope {
    /// Stable session identifier included in every hash.
    pub session_id: String,
    /// Strictly contiguous input sequence, starting at zero.
    pub input_seq: u64,
    /// State version the caller observed before submitting this input.
    pub expected_state_version: u64,
    /// Deterministic logical timestamp supplied by replay coordination.
    pub logical_ts_ns: i64,
    /// Stable input kind identifier.
    pub kind: String,
    /// Opaque canonical payload bytes owned by the domain layer.
    pub payload: Vec<u8>,
}

impl InputEnvelope {
    /// Returns a domain-separated canonical binary representation.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new();
        writer.tag(b"TRL-KERNEL-INPUT-v1\0");
        writer.text(&self.session_id);
        writer.u64(self.input_seq);
        writer.u64(self.expected_state_version);
        writer.i64(self.logical_ts_ns);
        writer.text(&self.kind);
        writer.bytes(&self.payload);
        writer.finish()
    }
}

/// Authoritative kernel event emitted for one accepted input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KernelEvent {
    /// Contiguous event sequence.
    pub event_seq: u64,
    /// State version after this event is applied.
    pub state_version: u64,
    /// Input logical time copied without consulting wall-clock time.
    pub logical_ts_ns: i64,
    /// Input kind copied as the event cause kind.
    pub kind: String,
    /// SHA-256 of the opaque input payload.
    pub payload_hash: Hash32,
    /// Hash of the prior accepted event or [`ZERO_HASH`] for the first event.
    pub prior_event_hash: Hash32,
    /// SHA-256 chain commitment for this event.
    pub current_event_hash: Hash32,
}

/// Minimal snapshot hook needed by later full simulator snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KernelSnapshot {
    /// Next required input sequence.
    pub next_input_seq: u64,
    /// Next event sequence.
    pub next_event_seq: u64,
    /// Current optimistic state version.
    pub state_version: u64,
    /// Current event-chain head.
    pub current_event_hash: Hash32,
}

/// Fail-closed kernel validation errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelError {
    /// Input sequence was not exactly the required next value.
    InputSequence { expected: u64, actual: u64 },
    /// Input expected a stale or future state version.
    StateVersion { expected: u64, actual: u64 },
    /// Restored snapshot violates kernel invariants.
    InvalidSnapshot,
    /// A sequence/version counter would overflow.
    CounterOverflow,
}

impl fmt::Display for KernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputSequence { expected, actual } => {
                write!(formatter, "input sequence mismatch: expected {expected}, received {actual}")
            }
            Self::StateVersion { expected, actual } => {
                write!(formatter, "state version mismatch: expected {expected}, received {actual}")
            }
            Self::InvalidSnapshot => formatter.write_str("invalid kernel snapshot"),
            Self::CounterOverflow => formatter.write_str("kernel counter overflow"),
        }
    }
}

impl std::error::Error for KernelError {}

/// Pure deterministic sequencing and event-chain state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Kernel {
    next_input_seq: u64,
    next_event_seq: u64,
    state_version: u64,
    current_event_hash: Hash32,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    /// Creates a pristine kernel at sequence/version zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_input_seq: 0,
            next_event_seq: 0,
            state_version: 0,
            current_event_hash: ZERO_HASH,
        }
    }

    /// Restores a previously committed kernel snapshot after checking invariants.
    ///
    /// # Errors
    /// Returns [`KernelError::InvalidSnapshot`] when counters cannot represent this
    /// one-event-per-input kernel history.
    pub fn from_snapshot(snapshot: KernelSnapshot) -> Result<Self, KernelError> {
        if snapshot.next_input_seq != snapshot.next_event_seq
            || snapshot.state_version != snapshot.next_input_seq
            || (snapshot.next_event_seq == 0 && snapshot.current_event_hash != ZERO_HASH)
            || (snapshot.next_event_seq > 0 && snapshot.current_event_hash == ZERO_HASH)
        {
            return Err(KernelError::InvalidSnapshot);
        }
        Ok(Self {
            next_input_seq: snapshot.next_input_seq,
            next_event_seq: snapshot.next_event_seq,
            state_version: snapshot.state_version,
            current_event_hash: snapshot.current_event_hash,
        })
    }

    /// Captures the deterministic sequencing/hash state for a later full snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> KernelSnapshot {
        KernelSnapshot {
            next_input_seq: self.next_input_seq,
            next_event_seq: self.next_event_seq,
            state_version: self.state_version,
            current_event_hash: self.current_event_hash,
        }
    }

    /// Returns the current optimistic state version.
    #[must_use]
    pub const fn state_version(&self) -> u64 {
        self.state_version
    }

    /// Applies exactly one input and emits exactly one chained kernel event.
    ///
    /// Validation and all overflow checks complete before kernel state mutates.
    ///
    /// # Errors
    /// Returns a stable validation error for non-contiguous sequence, wrong expected
    /// state version, or counter exhaustion.
    pub fn apply(&mut self, input: &InputEnvelope) -> Result<KernelEvent, KernelError> {
        if input.input_seq != self.next_input_seq {
            return Err(KernelError::InputSequence {
                expected: self.next_input_seq,
                actual: input.input_seq,
            });
        }
        if input.expected_state_version != self.state_version {
            return Err(KernelError::StateVersion {
                expected: self.state_version,
                actual: input.expected_state_version,
            });
        }

        let next_input_seq = self
            .next_input_seq
            .checked_add(1)
            .ok_or(KernelError::CounterOverflow)?;
        let next_event_seq = self
            .next_event_seq
            .checked_add(1)
            .ok_or(KernelError::CounterOverflow)?;
        let next_state_version = self
            .state_version
            .checked_add(1)
            .ok_or(KernelError::CounterOverflow)?;

        let payload_hash = sha256(&input.payload);
        let event_seq = self.next_event_seq;
        let prior_event_hash = self.current_event_hash;
        let mut writer = CanonicalWriter::new();
        writer.tag(b"TRL-KERNEL-EVENT-v1\0");
        writer.hash(&prior_event_hash);
        writer.u64(event_seq);
        writer.u64(next_state_version);
        writer.i64(input.logical_ts_ns);
        writer.text(&input.kind);
        writer.hash(&payload_hash);
        writer.bytes(&input.canonical_bytes());
        let current_event_hash = sha256(&writer.finish());

        let event = KernelEvent {
            event_seq,
            state_version: next_state_version,
            logical_ts_ns: input.logical_ts_ns,
            kind: input.kind.clone(),
            payload_hash,
            prior_event_hash,
            current_event_hash,
        };

        self.next_input_seq = next_input_seq;
        self.next_event_seq = next_event_seq;
        self.state_version = next_state_version;
        self.current_event_hash = current_event_hash;
        Ok(event)
    }

    /// Applies an ordered slice. Splitting the same ordered input stream into different
    /// batch boundaries yields identical events and final hashes.
    ///
    /// # Errors
    /// Returns the first validation error. Inputs accepted before that error remain
    /// committed exactly as they would when applying the same prefix individually.
    pub fn apply_batch(
        &mut self,
        inputs: &[InputEnvelope],
    ) -> Result<Vec<KernelEvent>, KernelError> {
        let mut events = Vec::with_capacity(inputs.len());
        for input in inputs {
            events.push(self.apply(input)?);
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(sequence: u64) -> InputEnvelope {
        InputEnvelope {
            session_id: "session-1".into(),
            input_seq: sequence,
            expected_state_version: sequence,
            logical_ts_ns: 1_000 + i64::try_from(sequence).unwrap(),
            kind: "MARKET_EVENT".into(),
            payload: format!("event-{sequence}").into_bytes(),
        }
    }

    #[test]
    fn repeated_runs_are_byte_deterministic() {
        let inputs = [input(0), input(1), input(2)];
        let mut left = Kernel::new();
        let mut right = Kernel::new();
        assert_eq!(left.apply_batch(&inputs).unwrap(), right.apply_batch(&inputs).unwrap());
        assert_eq!(left.snapshot(), right.snapshot());
    }

    #[test]
    fn repartitioned_input_stream_has_identical_chain() {
        let inputs = [input(0), input(1), input(2), input(3)];
        let mut batched = Kernel::new();
        let all_events = batched.apply_batch(&inputs).unwrap();

        let mut split = Kernel::new();
        let mut split_events = split.apply_batch(&inputs[..2]).unwrap();
        split_events.extend(split.apply_batch(&inputs[2..]).unwrap());

        assert_eq!(all_events, split_events);
        assert_eq!(batched.snapshot(), split.snapshot());
    }

    #[test]
    fn invalid_input_is_fail_closed() {
        let mut kernel = Kernel::new();
        let before = kernel.snapshot();
        let wrong_sequence = InputEnvelope { input_seq: 1, ..input(0) };
        assert_eq!(
            kernel.apply(&wrong_sequence),
            Err(KernelError::InputSequence { expected: 0, actual: 1 })
        );
        assert_eq!(kernel.snapshot(), before);

        let stale = InputEnvelope {
            expected_state_version: 9,
            ..input(0)
        };
        assert_eq!(
            kernel.apply(&stale),
            Err(KernelError::StateVersion { expected: 0, actual: 9 })
        );
        assert_eq!(kernel.snapshot(), before);
    }

    #[test]
    fn restore_continues_same_hash_chain() {
        let mut uninterrupted = Kernel::new();
        let first = uninterrupted.apply(&input(0)).unwrap();
        let snapshot = uninterrupted.snapshot();
        let second = uninterrupted.apply(&input(1)).unwrap();

        let mut restored = Kernel::from_snapshot(snapshot).unwrap();
        assert_eq!(restored.apply(&input(1)).unwrap(), second);
        assert_eq!(second.prior_event_hash, first.current_event_hash);
        assert_eq!(restored.snapshot(), uninterrupted.snapshot());
    }

    #[test]
    fn invalid_snapshot_fails_closed() {
        assert_eq!(
            Kernel::from_snapshot(KernelSnapshot {
                next_input_seq: 1,
                next_event_seq: 0,
                state_version: 1,
                current_event_hash: ZERO_HASH,
            }),
            Err(KernelError::InvalidSnapshot)
        );
    }
}
