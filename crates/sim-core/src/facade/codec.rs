//! Canonical domain-separated binary encoding for facade inputs.

use crate::economics::ScheduledEconomicId;
use crate::execution::f0::{Bar, F0Config, IntrabarPolicy};
use crate::execution::f1::{BboQuote, F1Config, TradePrint};
use crate::execution::f2::{BookSide, DepthLevel, L2Delta, L2Snapshot, SweepConfig};
use crate::hash::CanonicalWriter;
use crate::kernel::InputEnvelope;
use crate::numeric::{MoneyMinor, PriceAtoms, QtyAtoms};
use crate::orders::{NewOrder, OrderKind, ReplaceOrder, Side, TimeInForce, TopOfBook};

use super::types::{
    FacadeError, FacadeErrorCode, FacadeInput, FundingInput, ReplaceOrderInput, SubmitOrderInput,
};

const PAYLOAD_TAG: &[u8] = b"TRL-FACADE-PAYLOAD-v1\0";
const MAX_TEXT_BYTES: usize = 4096;
const MAX_DEPTH_LEVELS_PER_INPUT: usize = 100_000;

const KIND_ORDER_SUBMIT: &str = "ORDER_SUBMIT_V1";
const KIND_ORDER_CANCEL: &str = "ORDER_CANCEL_V1";
const KIND_ORDER_REPLACE: &str = "ORDER_REPLACE_V1";
const KIND_EXECUTE_F0: &str = "EXECUTE_F0_BAR_V1";
const KIND_EXECUTE_F1_QUOTE: &str = "EXECUTE_F1_QUOTE_V1";
const KIND_EXECUTE_F1_TRADE: &str = "EXECUTE_F1_TRADE_V1";
const KIND_F2_SNAPSHOT: &str = "F2_SNAPSHOT_V1";
const KIND_F2_DELTA: &str = "F2_DELTA_V1";
const KIND_EXECUTE_F2: &str = "EXECUTE_F2_V1";
const KIND_SET_LEVERAGE: &str = "RISK_SET_LEVERAGE_V1";
const KIND_EVALUATE_RISK: &str = "RISK_EVALUATE_V1";
const KIND_FUNDING: &str = "ECON_FUNDING_V1";

impl FacadeInput {
    /// Stable kernel input kind for this payload.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SubmitOrder(_) => KIND_ORDER_SUBMIT,
            Self::CancelOrder { .. } => KIND_ORDER_CANCEL,
            Self::ReplaceOrder(_) => KIND_ORDER_REPLACE,
            Self::ExecuteF0 { .. } => KIND_EXECUTE_F0,
            Self::ExecuteF1Quote { .. } => KIND_EXECUTE_F1_QUOTE,
            Self::ExecuteF1Trade { .. } => KIND_EXECUTE_F1_TRADE,
            Self::F2Snapshot(_) => KIND_F2_SNAPSHOT,
            Self::F2Delta(_) => KIND_F2_DELTA,
            Self::ExecuteF2 { .. } => KIND_EXECUTE_F2,
            Self::SetLeverage { .. } => KIND_SET_LEVERAGE,
            Self::EvaluateRisk { .. } => KIND_EVALUATE_RISK,
            Self::Funding(_) => KIND_FUNDING,
        }
    }

    /// Canonical domain-separated binary payload consumed by the simulator facade.
    #[must_use]
    pub fn canonical_payload(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new();
        writer.tag(PAYLOAD_TAG);
        match self {
            Self::SubmitOrder(input) => {
                write_new_order(&mut writer, &input.request);
                write_quote(&mut writer, input.quote);
            }
            Self::CancelOrder { order_id } => writer.u64(*order_id),
            Self::ReplaceOrder(input) => {
                writer.u64(input.order_id);
                writer.u64(input.replacement.quantity.get());
                write_order_kind(&mut writer, input.replacement.kind);
                write_quote(&mut writer, input.quote);
            }
            Self::ExecuteF0 {
                order_id,
                bar,
                config,
            } => {
                writer.u64(*order_id);
                writer.u64(bar.event_seq);
                writer.i64(bar.open.get());
                writer.i64(bar.high.get());
                writer.i64(bar.low.get());
                writer.i64(bar.close.get());
                writer.u64(bar.base_volume.get());
                match config.intrabar_policy {
                    IntrabarPolicy::Pessimistic => writer.text("PESSIMISTIC"),
                    IntrabarPolicy::Optimistic => writer.text("OPTIMISTIC"),
                    IntrabarPolicy::Seeded { seed } => {
                        writer.text("SEEDED");
                        writer.u64(seed);
                    }
                }
                writer.u64(config.market_slippage_atoms);
            }
            Self::ExecuteF1Quote {
                order_id,
                quote,
                config,
            } => {
                writer.u64(*order_id);
                write_bbo(&mut writer, *quote);
                write_f1_config(&mut writer, *config);
            }
            Self::ExecuteF1Trade {
                order_id,
                trade,
                eligible_after_event_seq,
                displayed_ahead,
                config,
            } => {
                writer.u64(*order_id);
                write_trade(&mut writer, *trade);
                writer.u64(*eligible_after_event_seq);
                writer.u64(displayed_ahead.get());
                write_f1_config(&mut writer, *config);
            }
            Self::F2Snapshot(snapshot) => {
                writer.u64(snapshot.sequence);
                write_levels(&mut writer, &snapshot.bids);
                write_levels(&mut writer, &snapshot.asks);
            }
            Self::F2Delta(delta) => {
                writer.u64(delta.previous_sequence);
                writer.u64(delta.sequence);
                writer.text(match delta.side {
                    BookSide::Bid => "BID",
                    BookSide::Ask => "ASK",
                });
                writer.i64(delta.price.get());
                writer.u64(delta.quantity.get());
            }
            Self::ExecuteF2 { order_id, config } => {
                writer.u64(*order_id);
                writer.u64(u64::try_from(config.max_levels).unwrap_or(u64::MAX));
                write_optional_qty(&mut writer, config.max_quantity);
            }
            Self::SetLeverage {
                leverage,
                equity,
                mark_price,
            } => {
                writer.u64(u64::from(*leverage));
                writer.i64(equity.get());
                writer.i64(mark_price.get());
            }
            Self::EvaluateRisk { equity, mark_price } => {
                writer.i64(equity.get());
                writer.i64(mark_price.get());
            }
            Self::Funding(input) => {
                writer.text(&input.id.source);
                writer.text(&input.id.event_id);
                writer.i64(input.cash_delta.get());
            }
        }
        writer.finish()
    }

    /// Builds the public kernel envelope without an alternate serialization path.
    #[must_use]
    pub fn envelope(
        &self,
        session_id: impl Into<String>,
        input_seq: u64,
        expected_state_version: u64,
        logical_ts_ns: i64,
    ) -> InputEnvelope {
        InputEnvelope {
            session_id: session_id.into(),
            input_seq,
            expected_state_version,
            logical_ts_ns,
            kind: self.kind().into(),
            payload: self.canonical_payload(),
        }
    }

    pub(crate) fn decode(kind: &str, bytes: &[u8]) -> Result<Self, FacadeError> {
        let mut reader = PayloadReader::new(bytes)?;
        let value = match kind {
            KIND_ORDER_SUBMIT => Self::SubmitOrder(SubmitOrderInput {
                request: read_new_order(&mut reader)?,
                quote: read_quote(&mut reader)?,
            }),
            KIND_ORDER_CANCEL => Self::CancelOrder {
                order_id: reader.u64()?,
            },
            KIND_ORDER_REPLACE => Self::ReplaceOrder(ReplaceOrderInput {
                order_id: reader.u64()?,
                replacement: ReplaceOrder {
                    quantity: QtyAtoms::new(reader.u64()?),
                    kind: read_order_kind(&mut reader)?,
                },
                quote: read_quote(&mut reader)?,
            }),
            KIND_EXECUTE_F0 => {
                let order_id = reader.u64()?;
                let bar = Bar {
                    event_seq: reader.u64()?,
                    open: PriceAtoms::new(reader.i64()?),
                    high: PriceAtoms::new(reader.i64()?),
                    low: PriceAtoms::new(reader.i64()?),
                    close: PriceAtoms::new(reader.i64()?),
                    base_volume: QtyAtoms::new(reader.u64()?),
                };
                let intrabar_policy = match reader.text()?.as_str() {
                    "PESSIMISTIC" => IntrabarPolicy::Pessimistic,
                    "OPTIMISTIC" => IntrabarPolicy::Optimistic,
                    "SEEDED" => IntrabarPolicy::Seeded {
                        seed: reader.u64()?,
                    },
                    _ => return Err(invalid_payload()),
                };
                Self::ExecuteF0 {
                    order_id,
                    bar,
                    config: F0Config {
                        intrabar_policy,
                        market_slippage_atoms: reader.u64()?,
                    },
                }
            }
            KIND_EXECUTE_F1_QUOTE => Self::ExecuteF1Quote {
                order_id: reader.u64()?,
                quote: read_bbo(&mut reader)?,
                config: read_f1_config(&mut reader)?,
            },
            KIND_EXECUTE_F1_TRADE => Self::ExecuteF1Trade {
                order_id: reader.u64()?,
                trade: read_trade(&mut reader)?,
                eligible_after_event_seq: reader.u64()?,
                displayed_ahead: QtyAtoms::new(reader.u64()?),
                config: read_f1_config(&mut reader)?,
            },
            KIND_F2_SNAPSHOT => Self::F2Snapshot(L2Snapshot {
                sequence: reader.u64()?,
                bids: read_levels(&mut reader)?,
                asks: read_levels(&mut reader)?,
            }),
            KIND_F2_DELTA => Self::F2Delta(L2Delta {
                previous_sequence: reader.u64()?,
                sequence: reader.u64()?,
                side: match reader.text()?.as_str() {
                    "BID" => BookSide::Bid,
                    "ASK" => BookSide::Ask,
                    _ => return Err(invalid_payload()),
                },
                price: PriceAtoms::new(reader.i64()?),
                quantity: QtyAtoms::new(reader.u64()?),
            }),
            KIND_EXECUTE_F2 => Self::ExecuteF2 {
                order_id: reader.u64()?,
                config: SweepConfig {
                    max_levels: usize::try_from(reader.u64()?).map_err(|_| invalid_payload())?,
                    max_quantity: read_optional_qty(&mut reader)?,
                },
            },
            KIND_SET_LEVERAGE => Self::SetLeverage {
                leverage: u8::try_from(reader.u64()?).map_err(|_| invalid_payload())?,
                equity: MoneyMinor::new(reader.i64()?),
                mark_price: PriceAtoms::new(reader.i64()?),
            },
            KIND_EVALUATE_RISK => Self::EvaluateRisk {
                equity: MoneyMinor::new(reader.i64()?),
                mark_price: PriceAtoms::new(reader.i64()?),
            },
            KIND_FUNDING => Self::Funding(FundingInput {
                id: ScheduledEconomicId::new(reader.text()?, reader.text()?)
                    .map_err(FacadeError::from_economics)?,
                cash_delta: MoneyMinor::new(reader.i64()?),
            }),
            _ => return Err(FacadeError::new(FacadeErrorCode::UnsupportedInputKind)),
        };
        reader.finish()?;
        Ok(value)
    }
}

fn write_new_order(writer: &mut CanonicalWriter, request: &NewOrder) {
    writer.text(&request.client_order_id);
    writer.text(&request.instrument_id);
    writer.text(match request.side {
        Side::Buy => "BUY",
        Side::Sell => "SELL",
    });
    writer.u64(request.quantity.get());
    write_order_kind(writer, request.kind);
    writer.text(match request.time_in_force {
        TimeInForce::Gtc => "GTC",
        TimeInForce::Ioc => "IOC",
        TimeInForce::Fok => "FOK",
    });
    write_bool(writer, request.reduce_only);
    write_bool(writer, request.post_only);
    write_bool(writer, request.marketable_only);
    writer.u64(request.submitted_at_event_seq);
}

fn read_new_order(reader: &mut PayloadReader<'_>) -> Result<NewOrder, FacadeError> {
    Ok(NewOrder {
        client_order_id: reader.text()?,
        instrument_id: reader.text()?,
        side: match reader.text()?.as_str() {
            "BUY" => Side::Buy,
            "SELL" => Side::Sell,
            _ => return Err(invalid_payload()),
        },
        quantity: QtyAtoms::new(reader.u64()?),
        kind: read_order_kind(reader)?,
        time_in_force: match reader.text()?.as_str() {
            "GTC" => TimeInForce::Gtc,
            "IOC" => TimeInForce::Ioc,
            "FOK" => TimeInForce::Fok,
            _ => return Err(invalid_payload()),
        },
        reduce_only: reader.boolean()?,
        post_only: reader.boolean()?,
        marketable_only: reader.boolean()?,
        submitted_at_event_seq: reader.u64()?,
    })
}

fn write_order_kind(writer: &mut CanonicalWriter, kind: OrderKind) {
    match kind {
        OrderKind::Market => writer.text("MARKET"),
        OrderKind::Limit { limit_price } => {
            writer.text("LIMIT");
            writer.i64(limit_price.get());
        }
        OrderKind::StopMarket { stop_price } => {
            writer.text("STOP_MARKET");
            writer.i64(stop_price.get());
        }
        OrderKind::StopLimit {
            stop_price,
            limit_price,
        } => {
            writer.text("STOP_LIMIT");
            writer.i64(stop_price.get());
            writer.i64(limit_price.get());
        }
    }
}

fn read_order_kind(reader: &mut PayloadReader<'_>) -> Result<OrderKind, FacadeError> {
    match reader.text()?.as_str() {
        "MARKET" => Ok(OrderKind::Market),
        "LIMIT" => Ok(OrderKind::Limit {
            limit_price: PriceAtoms::new(reader.i64()?),
        }),
        "STOP_MARKET" => Ok(OrderKind::StopMarket {
            stop_price: PriceAtoms::new(reader.i64()?),
        }),
        "STOP_LIMIT" => Ok(OrderKind::StopLimit {
            stop_price: PriceAtoms::new(reader.i64()?),
            limit_price: PriceAtoms::new(reader.i64()?),
        }),
        _ => Err(invalid_payload()),
    }
}

fn write_quote(writer: &mut CanonicalWriter, quote: Option<TopOfBook>) {
    write_bool(writer, quote.is_some());
    if let Some(quote) = quote {
        writer.i64(quote.bid.get());
        writer.i64(quote.ask.get());
    }
}

fn read_quote(reader: &mut PayloadReader<'_>) -> Result<Option<TopOfBook>, FacadeError> {
    if !reader.boolean()? {
        return Ok(None);
    }
    let bid = PriceAtoms::new(reader.i64()?);
    let ask = PriceAtoms::new(reader.i64()?);
    TopOfBook::new(bid, ask)
        .map(Some)
        .map_err(|_| invalid_payload())
}

fn write_bbo(writer: &mut CanonicalWriter, quote: BboQuote) {
    writer.u64(quote.event_seq);
    writer.i64(quote.event_time_ns);
    writer.i64(quote.bid.get());
    writer.u64(quote.bid_size.get());
    writer.i64(quote.ask.get());
    writer.u64(quote.ask_size.get());
}

fn read_bbo(reader: &mut PayloadReader<'_>) -> Result<BboQuote, FacadeError> {
    Ok(BboQuote {
        event_seq: reader.u64()?,
        event_time_ns: reader.i64()?,
        bid: PriceAtoms::new(reader.i64()?),
        bid_size: QtyAtoms::new(reader.u64()?),
        ask: PriceAtoms::new(reader.i64()?),
        ask_size: QtyAtoms::new(reader.u64()?),
    })
}

fn write_trade(writer: &mut CanonicalWriter, trade: TradePrint) {
    writer.u64(trade.event_seq);
    writer.i64(trade.event_time_ns);
    writer.i64(trade.price.get());
    writer.u64(trade.quantity.get());
}

fn read_trade(reader: &mut PayloadReader<'_>) -> Result<TradePrint, FacadeError> {
    Ok(TradePrint {
        event_seq: reader.u64()?,
        event_time_ns: reader.i64()?,
        price: PriceAtoms::new(reader.i64()?),
        quantity: QtyAtoms::new(reader.u64()?),
    })
}

fn write_f1_config(writer: &mut CanonicalWriter, config: F1Config) {
    writer.u64(config.max_quote_age_ns);
    writer.u64(config.max_trade_age_ns);
    write_optional_qty(writer, config.max_taker_fill);
    write_optional_qty(writer, config.max_maker_fill);
}

fn read_f1_config(reader: &mut PayloadReader<'_>) -> Result<F1Config, FacadeError> {
    Ok(F1Config {
        max_quote_age_ns: reader.u64()?,
        max_trade_age_ns: reader.u64()?,
        max_taker_fill: read_optional_qty(reader)?,
        max_maker_fill: read_optional_qty(reader)?,
    })
}

fn write_levels(writer: &mut CanonicalWriter, levels: &[DepthLevel]) {
    writer.u64(u64::try_from(levels.len()).unwrap_or(u64::MAX));
    for level in levels {
        writer.i64(level.price.get());
        writer.u64(level.quantity.get());
    }
}

fn read_levels(reader: &mut PayloadReader<'_>) -> Result<Vec<DepthLevel>, FacadeError> {
    let count = usize::try_from(reader.u64()?).map_err(|_| invalid_payload())?;
    if count > MAX_DEPTH_LEVELS_PER_INPUT {
        return Err(invalid_payload());
    }
    let mut levels = Vec::with_capacity(count);
    for _ in 0..count {
        levels.push(DepthLevel {
            price: PriceAtoms::new(reader.i64()?),
            quantity: QtyAtoms::new(reader.u64()?),
        });
    }
    Ok(levels)
}

fn write_optional_qty(writer: &mut CanonicalWriter, value: Option<QtyAtoms>) {
    write_bool(writer, value.is_some());
    if let Some(value) = value {
        writer.u64(value.get());
    }
}

fn read_optional_qty(reader: &mut PayloadReader<'_>) -> Result<Option<QtyAtoms>, FacadeError> {
    if reader.boolean()? {
        Ok(Some(QtyAtoms::new(reader.u64()?)))
    } else {
        Ok(None)
    }
}

fn write_bool(writer: &mut CanonicalWriter, value: bool) {
    writer.u64(if value { 1 } else { 0 });
}

fn invalid_payload() -> FacadeError {
    FacadeError::new(FacadeErrorCode::InvalidPayload)
}

struct PayloadReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, FacadeError> {
        if !bytes.starts_with(PAYLOAD_TAG) {
            return Err(invalid_payload());
        }
        Ok(Self {
            bytes,
            offset: PAYLOAD_TAG.len(),
        })
    }

    fn u64(&mut self) -> Result<u64, FacadeError> {
        let raw = self.take(8)?;
        Ok(u64::from_be_bytes(
            raw.try_into().map_err(|_| invalid_payload())?,
        ))
    }

    fn i64(&mut self) -> Result<i64, FacadeError> {
        let raw = self.take(8)?;
        Ok(i64::from_be_bytes(
            raw.try_into().map_err(|_| invalid_payload())?,
        ))
    }

    fn boolean(&mut self) -> Result<bool, FacadeError> {
        match self.u64()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid_payload()),
        }
    }

    fn text(&mut self) -> Result<String, FacadeError> {
        let length = usize::try_from(self.u64()?).map_err(|_| invalid_payload())?;
        if length > MAX_TEXT_BYTES {
            return Err(invalid_payload());
        }
        let raw = self.take(length)?;
        core::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| invalid_payload())
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], FacadeError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(invalid_payload)?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(invalid_payload)?;
        self.offset = end;
        Ok(result)
    }

    fn finish(self) -> Result<(), FacadeError> {
        if self.offset != self.bytes.len() {
            return Err(invalid_payload());
        }
        Ok(())
    }
}
