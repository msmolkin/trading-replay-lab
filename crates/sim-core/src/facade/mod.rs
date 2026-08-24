//! Versioned public simulator facade over authoritative domain state.

mod codec;
mod handlers;
mod types;

pub use types::{
    DomainEvent, DomainEventPayload, ExecutionFill, ExecutionModelRegistry, ExecutionTier,
    FACADE_API_VERSION, FacadeConfig, FacadeError, FacadeErrorCode, FacadeInitialState,
    FacadeInput, FacadeRules, FundingInput, ReplaceOrderInput, SubmitOrderInput,
};

use crate::economics::EconomicsState;
use crate::execution::f2::{F2Error, L2Book, L2Delta};
use crate::kernel::{InputEnvelope, Kernel};
use crate::ledger::Ledger;
use crate::orders::OrderState;
use crate::positions::Position;
use crate::risk::RiskState;
use crate::snapshot::{SNAPSHOT_FORMAT_VERSION, SimulatorSnapshot};

/// Versioned deterministic simulator facade.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorFacade {
    config: FacadeConfig,
    kernel: Kernel,
    last_logical_ts_ns: Option<i64>,
    market_event_seq: Option<u64>,
    orders: OrderState,
    position: Position,
    ledger: Ledger,
    economics: EconomicsState,
    risk: RiskState,
    f2_book: Option<L2Book>,
}

impl SimulatorFacade {
    /// Creates a pristine facade after validating all forgeable initial state.
    ///
    /// # Errors
    /// Returns a stable configuration/snapshot validation error without creating partial state.
    pub fn new(config: FacadeConfig, initial: FacadeInitialState) -> Result<Self, FacadeError> {
        config.validate()?;
        let position =
            Position::from_snapshot(initial.position).map_err(FacadeError::from_position)?;
        let ledger = Ledger::from_snapshot(initial.ledger).map_err(FacadeError::from_ledger)?;
        let f2_book = (config.execution_tier == ExecutionTier::F2).then(L2Book::new);
        Ok(Self {
            config,
            kernel: Kernel::new(),
            last_logical_ts_ns: None,
            market_event_seq: None,
            orders: OrderState::new(),
            position,
            ledger,
            economics: EconomicsState::new(),
            risk: RiskState::new(initial.leverage),
            f2_book,
        })
    }

    /// Restores complete deterministic continuation state and revalidates public snapshot parts.
    ///
    /// # Errors
    /// Returns [`FacadeErrorCode::InvalidSnapshot`] or a stable compatibility error without
    /// producing a partially restored facade.
    pub fn from_snapshot(snapshot: SimulatorSnapshot) -> Result<Self, FacadeError> {
        if snapshot.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(FacadeError::new(
                FacadeErrorCode::UnsupportedSnapshotVersion,
            ));
        }
        snapshot.config.validate()?;
        let kernel = Kernel::from_snapshot(snapshot.kernel)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let position = Position::from_snapshot(snapshot.position)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let ledger = Ledger::from_snapshot(snapshot.ledger)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let has_inputs = kernel.state_version() > 0;
        if has_inputs != snapshot.last_logical_ts_ns.is_some() {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        if snapshot
            .orders
            .iter()
            .any(|order| order.instrument_id != snapshot.config.instrument_id)
        {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        validate_f2_snapshot_state(
            snapshot.config.execution_tier,
            snapshot.market_event_seq,
            snapshot.f2_book.as_ref(),
        )?;
        Ok(Self {
            config: snapshot.config,
            kernel,
            last_logical_ts_ns: snapshot.last_logical_ts_ns,
            market_event_seq: snapshot.market_event_seq,
            orders: snapshot.orders,
            position,
            ledger,
            economics: snapshot.economics,
            risk: snapshot.risk,
            f2_book: snapshot.f2_book,
        })
    }

    /// Captures complete deterministic continuation state.
    #[must_use]
    pub fn snapshot(&self) -> SimulatorSnapshot {
        SimulatorSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            config: self.config.clone(),
            last_logical_ts_ns: self.last_logical_ts_ns,
            market_event_seq: self.market_event_seq,
            kernel: self.kernel.snapshot(),
            orders: self.orders.clone(),
            position: self.position,
            ledger: self.ledger.snapshot(),
            economics: self.economics.clone(),
            risk: self.risk,
            f2_book: self.f2_book.clone(),
        }
    }

    /// Current optimistic state version for constructing the next input envelope.
    #[must_use]
    pub const fn state_version(&self) -> u64 {
        self.kernel.state_version()
    }

    /// Current authoritative position.
    #[must_use]
    pub const fn position(&self) -> Position {
        self.position
    }

    /// Current authoritative order collection.
    #[must_use]
    pub const fn orders(&self) -> &OrderState {
        &self.orders
    }

    /// Current balanced ledger.
    #[must_use]
    pub const fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// Current risk/leverage/liquidation state.
    #[must_use]
    pub const fn risk(&self) -> RiskState {
        self.risk
    }

    /// Current F2 book when the session is F2.
    #[must_use]
    pub const fn f2_book(&self) -> Option<&L2Book> {
        self.f2_book.as_ref()
    }

    /// Applies one public input atomically and returns deterministic typed domain events.
    ///
    /// The complete facade is cloned first. Kernel sequence/version/hash validation and all
    /// domain transitions occur on the clone. Ordinary invalid inputs roll back completely.
    /// A malformed or discontinuous F2 delta is different: once the depth model detects that
    /// reconstruction is untrustworthy, that historical input is consumed and the quarantined
    /// disabled-book state is committed so stale depth can never become executable again.
    ///
    /// # Errors
    /// Returns a stable [`FacadeErrorCode`] with no state mutation, except for authoritative F2
    /// depth invalidation described above.
    pub fn apply(&mut self, input: &InputEnvelope) -> Result<Vec<DomainEvent>, FacadeError> {
        if input.session_id != self.config.session_id {
            return Err(FacadeError::new(FacadeErrorCode::SessionMismatch));
        }
        if self
            .last_logical_ts_ns
            .is_some_and(|last| input.logical_ts_ns < last)
        {
            return Err(FacadeError::new(FacadeErrorCode::LogicalTimeRegression));
        }
        let command = FacadeInput::decode(&input.kind, &input.payload)?;
        let f2_delta_context = match &command {
            FacadeInput::F2Delta(delta) => {
                Some((*delta, self.f2_book().is_some_and(L2Book::is_enabled)))
            }
            _ => None,
        };
        let mut next = self.clone();
        let cause = next.kernel.apply(input).map_err(FacadeError::from_kernel)?;
        let dispatch = next.dispatch(command, &cause, input.logical_ts_ns);
        let events = match dispatch {
            Ok(events) => events,
            Err(error) => {
                let Some((delta, was_enabled)) = f2_delta_context else {
                    return Err(error);
                };
                if error.code != FacadeErrorCode::F2Execution
                    || next.f2_book().is_none_or(L2Book::is_enabled)
                {
                    return Err(error);
                }
                let reason = classify_depth_invalidation(&delta, next.f2_book(), was_enabled);
                vec![DomainEvent {
                    cause: cause.clone(),
                    ordinal: 0,
                    payload: DomainEventPayload::DepthInvalidated {
                        sequence: delta.sequence,
                        reason,
                    },
                }]
            }
        };
        next.last_logical_ts_ns = Some(input.logical_ts_ns);
        *self = next;
        Ok(events)
    }
}

fn classify_depth_invalidation(
    delta: &L2Delta,
    book: Option<&L2Book>,
    was_enabled: bool,
) -> F2Error {
    if !was_enabled {
        return F2Error::BookDisabled;
    }
    if delta.price.get() <= 0 {
        return F2Error::InvalidBook;
    }
    if book.is_none_or(|book| {
        book.sequence().is_none_or(|sequence| {
            delta.previous_sequence != sequence || delta.sequence <= delta.previous_sequence
        })
    }) {
        return F2Error::SequenceGap;
    }
    F2Error::InvalidBook
}

fn validate_f2_snapshot_state(
    tier: ExecutionTier,
    market_event_seq: Option<u64>,
    book: Option<&L2Book>,
) -> Result<(), FacadeError> {
    match (tier, book) {
        (ExecutionTier::F2, Some(book)) if book.is_enabled() => {
            if market_event_seq.is_none() || book.sequence() != market_event_seq {
                return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
            }
        }
        (ExecutionTier::F2, Some(book)) => {
            if let Some(sequence) = book.sequence() {
                let Some(frontier) = market_event_seq else {
                    return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
                };
                if sequence > frontier {
                    return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
                }
            }
        }
        (ExecutionTier::F2, None) => {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        (_, Some(_)) => return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot)),
        (_, None) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
