"""Deterministic quality validation and immutable dataset status decisions."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from enum import IntEnum
from typing import Mapping, Sequence


class Severity(IntEnum):
    """Issue severity used to derive immutable dataset status."""

    INFO = 0
    DEGRADED = 1
    QUARANTINED = 2


@dataclass(frozen=True, slots=True)
class QualityIssue:
    """Stable machine-readable quality finding."""

    code: str
    severity: Severity
    event_index: int | None
    detail: str


@dataclass(frozen=True, slots=True)
class QualityPolicy:
    """Declared quality thresholds and point-in-time instrument increments."""

    tick_size_atoms: int
    qty_increment_atoms: int
    max_gap_ns: int
    max_quote_staleness_ns: int

    def __post_init__(self) -> None:
        if self.tick_size_atoms <= 0 or self.qty_increment_atoms <= 0:
            raise ValueError("increments must be positive")
        if self.max_gap_ns < 0 or self.max_quote_staleness_ns < 0:
            raise ValueError("gap thresholds cannot be negative")


@dataclass(frozen=True, slots=True)
class QualityReport:
    """Immutable, reproducibly hashed quality decision."""

    status: str
    row_count: int
    min_event_ts_ns: int | None
    max_event_ts_ns: int | None
    duplicates: int
    issues: tuple[QualityIssue, ...]
    decision_hash: str


def _wire_int(value: object) -> int:
    if not isinstance(value, str) or not value:
        raise ValueError("exact integer must be a canonical decimal string")
    if value == "-0" or value.startswith("+") or (len(value) > 1 and value.startswith("0")):
        raise ValueError("non-canonical decimal integer")
    if value.startswith("-"):
        digits = value[1:]
        if not digits or (len(digits) > 1 and digits.startswith("0")):
            raise ValueError("non-canonical decimal integer")
    else:
        digits = value
    if not digits.isascii() or not digits.isdigit():
        raise ValueError("non-canonical decimal integer")
    return int(value, 10)


def _payload(event: Mapping[str, object]) -> Mapping[str, object]:
    value = event.get("payload")
    if not isinstance(value, dict):
        raise ValueError("payload must be an object")
    return value


def _levels(payload: Mapping[str, object], side: str) -> list[tuple[int, int]]:
    raw = payload.get(side)
    if not isinstance(raw, list):
        raise ValueError(f"{side} must be an array")
    levels: list[tuple[int, int]] = []
    for level in raw:
        if not isinstance(level, dict):
            raise ValueError("book level must be an object")
        levels.append((_wire_int(level.get("price_atoms")), _wire_int(level.get("qty_atoms"))))
    return levels


def _increment_issues(
    payload: Mapping[str, object], policy: QualityPolicy, index: int
) -> list[QualityIssue]:
    issues: list[QualityIssue] = []

    def check_mapping(value: Mapping[str, object]) -> None:
        for key, item in value.items():
            if key.endswith("price_atoms"):
                try:
                    atoms = _wire_int(item)
                except ValueError as error:
                    issues.append(QualityIssue("INVALID_INTEGER", Severity.QUARANTINED, index, str(error)))
                else:
                    if not abs(atoms) % policy.tick_size_atoms == 0:
                        issues.append(
                            QualityIssue(
                                "PRICE_INCREMENT",
                                Severity.QUARANTINED,
                                index,
                                f"{key}={atoms} is not aligned to {policy.tick_size_atoms}",
                            )
                        )
            elif key.endswith("qty_atoms") or key.endswith("volume_atoms"):
                try:
                    atoms = _wire_int(item)
                except ValueError as error:
                    issues.append(QualityIssue("INVALID_INTEGER", Severity.QUARANTINED, index, str(error)))
                else:
                    if atoms < 0 or atoms % policy.qty_increment_atoms != 0:
                        issues.append(
                            QualityIssue(
                                "QUANTITY_INCREMENT",
                                Severity.QUARANTINED,
                                index,
                                f"{key}={atoms} is not aligned to {policy.qty_increment_atoms}",
                            )
                        )
            elif isinstance(item, dict):
                check_mapping(item)
            elif isinstance(item, list):
                for nested in item:
                    if isinstance(nested, dict):
                        check_mapping(nested)

    check_mapping(payload)
    return issues


def _decision_hash(
    *, status: str, row_count: int, minimum: int | None, maximum: int | None, duplicates: int, issues: Sequence[QualityIssue]
) -> str:
    document = {
        "duplicates": duplicates,
        "issues": [
            {
                "code": issue.code,
                "detail": issue.detail,
                "event_index": issue.event_index,
                "severity": int(issue.severity),
            }
            for issue in issues
        ],
        "max_event_ts_ns": maximum,
        "min_event_ts_ns": minimum,
        "row_count": row_count,
        "status": status,
    }
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(encoded).hexdigest()


def validate_events(
    events: Sequence[Mapping[str, object]], policy: QualityPolicy
) -> QualityReport:
    """Validate canonical events and derive a reproducible immutable status decision."""
    issues: list[QualityIssue] = []
    seen: set[bytes] = set()
    duplicates = 0
    minimum: int | None = None
    maximum: int | None = None
    prior_ts: int | None = None
    prior_sequence: int | None = None
    prior_quote_ts: int | None = None
    book_ready = False

    for index, event in enumerate(events):
        encoded = json.dumps(event, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        if encoded in seen:
            duplicates += 1
            issues.append(QualityIssue("DUPLICATE", Severity.DEGRADED, index, "exact duplicate event"))
        else:
            seen.add(encoded)

        try:
            ts = _wire_int(event.get("ts_event_ns"))
        except ValueError as error:
            issues.append(QualityIssue("INVALID_TIMESTAMP", Severity.QUARANTINED, index, str(error)))
            continue
        minimum = ts if minimum is None else min(minimum, ts)
        maximum = ts if maximum is None else max(maximum, ts)
        if prior_ts is not None:
            if ts < prior_ts:
                issues.append(QualityIssue("OUT_OF_ORDER", Severity.QUARANTINED, index, "event timestamp regressed"))
            elif ts - prior_ts > policy.max_gap_ns:
                issues.append(
                    QualityIssue(
                        "TIME_GAP",
                        Severity.DEGRADED,
                        index,
                        f"gap {ts - prior_ts}ns exceeds {policy.max_gap_ns}ns",
                    )
                )
        prior_ts = ts

        raw_sequence = event.get("source_sequence")
        sequence: int | None = None
        if raw_sequence is not None:
            try:
                sequence = _wire_int(raw_sequence)
            except ValueError as error:
                issues.append(QualityIssue("INVALID_SEQUENCE", Severity.QUARANTINED, index, str(error)))
            else:
                if prior_sequence is not None and sequence != prior_sequence + 1:
                    kind = event.get("kind")
                    severity = Severity.QUARANTINED if kind in {"BOOK_DELTA_L2", "BOOK_SNAPSHOT_L2"} else Severity.DEGRADED
                    code = "BOOK_SEQUENCE_GAP" if severity is Severity.QUARANTINED else "SOURCE_SEQUENCE_GAP"
                    issues.append(QualityIssue(code, severity, index, f"expected {prior_sequence + 1}, received {sequence}"))
                prior_sequence = sequence

        kind = event.get("kind")
        try:
            payload = _payload(event)
        except ValueError as error:
            issues.append(QualityIssue("INVALID_PAYLOAD", Severity.QUARANTINED, index, str(error)))
            continue
        issues.extend(_increment_issues(payload, policy, index))

        if kind == "BOOK_SNAPSHOT_L2":
            try:
                bids = _levels(payload, "bids")
                asks = _levels(payload, "asks")
            except ValueError as error:
                issues.append(QualityIssue("INVALID_BOOK", Severity.QUARANTINED, index, str(error)))
            else:
                book_ready = True
                if bids and asks and max(price for price, _ in bids) >= min(price for price, _ in asks):
                    issues.append(QualityIssue("CROSSED_BOOK", Severity.QUARANTINED, index, "snapshot bid crosses ask"))
        elif kind == "BOOK_DELTA_L2" and not book_ready:
            issues.append(QualityIssue("DELTA_WITHOUT_SNAPSHOT", Severity.QUARANTINED, index, "L2 delta cannot be reconstructed"))
        elif kind == "BBO":
            bid = payload.get("bid_price_atoms")
            ask = payload.get("ask_price_atoms")
            if bid is not None and ask is not None:
                try:
                    crossed = _wire_int(bid) >= _wire_int(ask)
                except ValueError as error:
                    issues.append(QualityIssue("INVALID_BBO", Severity.QUARANTINED, index, str(error)))
                else:
                    if crossed:
                        issues.append(QualityIssue("CROSSED_BBO", Severity.DEGRADED, index, "bid is not below ask"))
            if prior_quote_ts is not None and ts - prior_quote_ts > policy.max_quote_staleness_ns:
                issues.append(QualityIssue("STALE_QUOTE_INTERVAL", Severity.DEGRADED, index, "BBO update interval exceeded threshold"))
            prior_quote_ts = ts

    highest = max((issue.severity for issue in issues), default=Severity.INFO)
    status = "QUARANTINED" if highest is Severity.QUARANTINED else "DEGRADED" if highest is Severity.DEGRADED else "VALID"
    immutable_issues = tuple(issues)
    return QualityReport(
        status=status,
        row_count=len(events),
        min_event_ts_ns=minimum,
        max_event_ts_ns=maximum,
        duplicates=duplicates,
        issues=immutable_issues,
        decision_hash=_decision_hash(
            status=status,
            row_count=len(events),
            minimum=minimum,
            maximum=maximum,
            duplicates=duplicates,
            issues=immutable_issues,
        ),
    )


__all__ = [
    "QualityIssue",
    "QualityPolicy",
    "QualityReport",
    "Severity",
    "validate_events",
]
