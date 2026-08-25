"""Validation, quote resolution, and persistence orchestration for trading commands."""

from __future__ import annotations

import time
from collections.abc import Mapping
from dataclasses import dataclass

from .model import (
    AcceptedCommand,
    Clock,
    CommandErrorCode,
    CommandServiceError,
    CommandType,
    PreparedCommand,
    PriceReference,
    VisibleQuote,
    VisibleQuoteResolver,
    canonical_i64_text,
    canonical_payload_hash,
    canonical_u64_text,
    optional_bool,
    reject_unknown,
    require_string,
)
from .store import CommandStore

_ORDER_FIELDS = frozenset(
    {
        "instrument_id",
        "side",
        "quantity_atoms",
        "order_type",
        "limit_price_atoms",
        "stop_price_atoms",
        "price_reference",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
    }
)
_REPLACE_FIELDS = frozenset(
    {
        "quantity_atoms",
        "limit_price_atoms",
        "stop_price_atoms",
        "price_reference",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
    }
)


@dataclass(frozen=True, slots=True)
class SystemClock:
    """Wall clock used only for non-authoritative API receipt metadata."""

    def now_ns(self) -> int:
        return time.time_ns()


class TradingCommandService:
    """Canonicalize trading intent before the replay coordinator sees it."""

    def __init__(
        self,
        *,
        store: CommandStore,
        quote_resolver: VisibleQuoteResolver,
        clock: Clock | None = None,
    ) -> None:
        self.store = store
        self.quote_resolver = quote_resolver
        self.clock = SystemClock() if clock is None else clock

    def submit_order(
        self,
        *,
        session_id: str,
        principal_id: str,
        idempotency_key: str,
        expected_session_version: int,
        request: Mapping[str, object],
    ) -> AcceptedCommand:
        """Validate and accept one new order command."""
        prepared = self._prepare_order(session_id, principal_id, request)
        return self._accept(
            session_id=session_id,
            principal_id=principal_id,
            idempotency_key=idempotency_key,
            expected_session_version=expected_session_version,
            prepared=prepared,
        )

    def cancel_order(
        self,
        *,
        session_id: str,
        principal_id: str,
        idempotency_key: str,
        expected_session_version: int,
        order_id: str,
        request: Mapping[str, object],
    ) -> AcceptedCommand:
        """Accept a cancel without allowing client-supplied authoritative fields."""
        reject_unknown(request, frozenset())
        _safe_identifier(order_id, "order_id")
        payload: dict[str, object] = {
            "command_type": CommandType.CANCEL_ORDER.value,
            "order_id": order_id,
        }
        return self._accept_payload(
            session_id=session_id,
            principal_id=principal_id,
            idempotency_key=idempotency_key,
            expected_session_version=expected_session_version,
            payload=payload,
        )

    def replace_order(
        self,
        *,
        session_id: str,
        principal_id: str,
        idempotency_key: str,
        expected_session_version: int,
        order_id: str,
        request: Mapping[str, object],
    ) -> AcceptedCommand:
        """Accept a canonical replacement for an existing order identity."""
        _safe_identifier(order_id, "order_id")
        reject_unknown(request, _REPLACE_FIELDS)
        if not request:
            raise _invalid("replacement must change at least one field")
        payload: dict[str, object] = {
            "command_type": CommandType.REPLACE_ORDER.value,
            "order_id": order_id,
        }
        self._copy_order_mutations(
            payload,
            session_id=session_id,
            principal_id=principal_id,
            request=request,
        )
        return self._accept_payload(
            session_id=session_id,
            principal_id=principal_id,
            idempotency_key=idempotency_key,
            expected_session_version=expected_session_version,
            payload=payload,
        )

    def set_leverage(
        self,
        *,
        session_id: str,
        principal_id: str,
        idempotency_key: str,
        expected_session_version: int,
        request: Mapping[str, object],
    ) -> AcceptedCommand:
        """Accept a bounded 1x-50x leverage request; leverage is dimensionless, not money."""
        reject_unknown(request, frozenset({"leverage"}))
        leverage = request.get("leverage")
        if isinstance(leverage, bool) or not isinstance(leverage, int) or not 1 <= leverage <= 50:
            raise _invalid("leverage must be an integer from 1 through 50")
        return self._accept_payload(
            session_id=session_id,
            principal_id=principal_id,
            idempotency_key=idempotency_key,
            expected_session_version=expected_session_version,
            payload={
                "command_type": CommandType.SET_LEVERAGE.value,
                "leverage": leverage,
            },
        )

    def get_command(
        self,
        *,
        session_id: str,
        principal_id: str,
        command_id: str,
    ) -> AcceptedCommand:
        """Return one accepted command within the authenticated principal boundary."""
        return self.store.get(
            session_id=session_id,
            principal_id=principal_id,
            command_id=command_id,
        )

    def _prepare_order(
        self,
        session_id: str,
        principal_id: str,
        request: Mapping[str, object],
    ) -> PreparedCommand:
        reject_unknown(request, _ORDER_FIELDS)
        instrument_id = require_string(request, "instrument_id")
        side = _enum_string(request, "side", frozenset({"BUY", "SELL"}))
        quantity = canonical_u64_text(
            request.get("quantity_atoms"), "quantity_atoms", positive=True
        )
        order_type = _enum_string(
            request,
            "order_type",
            frozenset({"MARKET", "LIMIT", "STOP_MARKET", "STOP_LIMIT"}),
        )
        time_in_force = _enum_string(
            request,
            "time_in_force",
            frozenset({"GTC", "IOC", "FOK"}),
        )
        reduce_only = optional_bool(request, "reduce_only")
        post_only = optional_bool(request, "post_only")
        marketable_only = optional_bool(request, "marketable_only")
        if post_only and order_type not in {"LIMIT", "STOP_LIMIT"}:
            raise _invalid("post_only requires a limit-bearing order type")

        payload: dict[str, object] = {
            "command_type": CommandType.SUBMIT_ORDER.value,
            "instrument_id": instrument_id,
            "side": side,
            "quantity_atoms": quantity,
            "order_type": order_type,
            "time_in_force": time_in_force,
            "reduce_only": reduce_only,
            "post_only": post_only,
            "marketable_only": marketable_only,
        }
        self._resolve_prices(
            payload,
            session_id=session_id,
            principal_id=principal_id,
            side=side,
            order_type=order_type,
            request=request,
        )
        return PreparedCommand(payload, canonical_payload_hash(payload))

    def _copy_order_mutations(
        self,
        payload: dict[str, object],
        *,
        session_id: str,
        principal_id: str,
        request: Mapping[str, object],
    ) -> None:
        if "quantity_atoms" in request:
            payload["quantity_atoms"] = canonical_u64_text(
                request.get("quantity_atoms"),
                "quantity_atoms",
                positive=True,
            )
        for name in ("time_in_force",):
            if name in request:
                payload[name] = _enum_string(request, name, frozenset({"GTC", "IOC", "FOK"}))
        for name in ("reduce_only", "post_only", "marketable_only"):
            if name in request:
                payload[name] = optional_bool(request, name)

        has_price = any(
            name in request for name in ("limit_price_atoms", "stop_price_atoms", "price_reference")
        )
        if has_price:
            side = _enum_string(request, "side", frozenset({"BUY", "SELL"}), required=False)
            if "price_reference" in request and side is None:
                raise _invalid(
                    "replacement price_reference requires side for passive midpoint rounding"
                )
            self._resolve_replacement_prices(
                payload,
                session_id=session_id,
                principal_id=principal_id,
                side=side,
                request=request,
            )

    def _resolve_prices(
        self,
        payload: dict[str, object],
        *,
        session_id: str,
        principal_id: str,
        side: str,
        order_type: str,
        request: Mapping[str, object],
    ) -> None:
        reference = request.get("price_reference")
        limit_raw = request.get("limit_price_atoms")
        stop_raw = request.get("stop_price_atoms")
        if reference is not None and limit_raw is not None:
            raise _invalid("provide either price_reference or limit_price_atoms, not both")
        if reference is not None:
            if order_type not in {"LIMIT", "STOP_LIMIT"}:
                raise _invalid("price_reference requires a limit-bearing order type")
            resolved, event_id, reference_name = self._resolve_reference(
                session_id,
                principal_id,
                side,
                reference,
            )
            payload["limit_price_atoms"] = str(resolved)
            payload["price_reference"] = reference_name
            payload["quote_event_id"] = event_id
        elif order_type in {"LIMIT", "STOP_LIMIT"}:
            payload["limit_price_atoms"] = canonical_i64_text(
                limit_raw,
                "limit_price_atoms",
                positive=True,
            )
        elif limit_raw is not None:
            raise _invalid("limit_price_atoms is incompatible with this order_type")

        if order_type in {"STOP_MARKET", "STOP_LIMIT"}:
            payload["stop_price_atoms"] = canonical_i64_text(
                stop_raw,
                "stop_price_atoms",
                positive=True,
            )
        elif stop_raw is not None:
            raise _invalid("stop_price_atoms is incompatible with this order_type")

    def _resolve_replacement_prices(
        self,
        payload: dict[str, object],
        *,
        session_id: str,
        principal_id: str,
        side: str | None,
        request: Mapping[str, object],
    ) -> None:
        reference = request.get("price_reference")
        limit_raw = request.get("limit_price_atoms")
        if reference is not None and limit_raw is not None:
            raise _invalid("provide either price_reference or limit_price_atoms, not both")
        if reference is not None:
            assert side is not None
            resolved, event_id, reference_name = self._resolve_reference(
                session_id,
                principal_id,
                side,
                reference,
            )
            payload["limit_price_atoms"] = str(resolved)
            payload["price_reference"] = reference_name
            payload["quote_event_id"] = event_id
        elif limit_raw is not None:
            payload["limit_price_atoms"] = canonical_i64_text(
                limit_raw,
                "limit_price_atoms",
                positive=True,
            )
        if "stop_price_atoms" in request:
            payload["stop_price_atoms"] = canonical_i64_text(
                request.get("stop_price_atoms"),
                "stop_price_atoms",
                positive=True,
            )

    def _resolve_reference(
        self,
        session_id: str,
        principal_id: str,
        side: str,
        raw_reference: object,
    ) -> tuple[int, str, str]:
        if not isinstance(raw_reference, str):
            raise _invalid("price_reference must be BID, ASK, or MIDPOINT")
        try:
            reference = PriceReference(raw_reference)
        except ValueError as error:
            raise _invalid("price_reference must be BID, ASK, or MIDPOINT") from error
        quote = self.quote_resolver.current_quote(
            session_id=session_id,
            principal_id=principal_id,
        )
        if quote is None:
            raise CommandServiceError(
                CommandErrorCode.QUOTE_UNAVAILABLE,
                "no visible quote is available at the replay frontier",
            )
        return _reference_price(quote, side, reference), quote.event_id, reference.value

    def _accept_payload(
        self,
        *,
        session_id: str,
        principal_id: str,
        idempotency_key: str,
        expected_session_version: int,
        payload: dict[str, object],
    ) -> AcceptedCommand:
        return self._accept(
            session_id=session_id,
            principal_id=principal_id,
            idempotency_key=idempotency_key,
            expected_session_version=expected_session_version,
            prepared=PreparedCommand(payload, canonical_payload_hash(payload)),
        )

    def _accept(
        self,
        *,
        session_id: str,
        principal_id: str,
        idempotency_key: str,
        expected_session_version: int,
        prepared: PreparedCommand,
    ) -> AcceptedCommand:
        return self.store.accept(
            session_id=session_id,
            principal_id=principal_id,
            idempotency_key=idempotency_key,
            expected_session_version=expected_session_version,
            accepted_at_ns=self.clock.now_ns(),
            prepared=prepared,
        )


def _reference_price(quote: VisibleQuote, side: str, reference: PriceReference) -> int:
    if reference is PriceReference.BID:
        return quote.bid_price_atoms
    if reference is PriceReference.ASK:
        return quote.ask_price_atoms
    total = quote.bid_price_atoms + quote.ask_price_atoms
    if side == "BUY":
        return total // 2
    return (total + 1) // 2


def _enum_string(
    fields: Mapping[str, object],
    name: str,
    allowed: frozenset[str],
    *,
    required: bool = True,
) -> str | None:
    value = fields.get(name)
    if value is None and not required:
        return None
    if not isinstance(value, str) or value not in allowed:
        raise _invalid(f"{name} must be one of {', '.join(sorted(allowed))}")
    return value


def _safe_identifier(value: str, name: str) -> None:
    if not value or any(character in value for character in "\x00\r\n"):
        raise _invalid(f"{name} is invalid")


def _invalid(message: str) -> CommandServiceError:
    return CommandServiceError(CommandErrorCode.INVALID_COMMAND, message)


__all__ = ["SystemClock", "TradingCommandService"]
