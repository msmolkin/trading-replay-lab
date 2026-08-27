from __future__ import annotations

import gzip
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path

import pytest

from trading_replay_ingest.adapters.tardis import (
    TardisAdapter,
    TardisConfig,
    TardisCoverage,
    TardisDataType,
    TardisEntitlement,
    TardisRequestError,
    TardisSchemaError,
    capabilities_for_interval,
)
from trading_replay_ingest.core import (
    BudgetGuard,
    ContentAddressedCache,
    FetchRequest,
    IngestionRunner,
    JobStateStore,
)

DAY_NS = 86_400_000_000_000


@dataclass(slots=True)
class FakeTransport:
    responses: dict[str, bytes]
    fail_once_on: str | None = None
    failed: bool = False
    calls: list[tuple[str, str | None]] = field(default_factory=list)

    def get(self, url: str, max_bytes: int, bearer_token: str | None) -> bytes:
        self.calls.append((url, bearer_token))
        if url == self.fail_once_on and not self.failed:
            self.failed = True
            raise RuntimeError("synthetic Tardis interruption")
        payload = self.responses[url]
        if len(payload) > max_bytes:
            raise AssertionError("test response exceeds configured ceiling")
        return payload


def coverage(*data_types: TardisDataType) -> TardisCoverage:
    return TardisCoverage(
        exchange="deribit",
        symbol="BTC-PERPETUAL",
        data_types=frozenset(data_types),
        available_since=date(2020, 1, 1),
        available_to=date(2020, 1, 31),
        exported_until=date(2020, 1, 31),
    )


def config(data_type: TardisDataType) -> TardisConfig:
    return TardisConfig(
        exchange="deribit",
        symbol="BTC-PERPETUAL",
        data_type=data_type,
        instrument_id="DERIBIT:BTC-PERPETUAL",
        dataset_id=f"tardis:{data_type.value}",
        price_scale=2,
        qty_scale=3,
        rate_scale=9,
    )


def entitlement(*data_types: TardisDataType, sample_only: bool = False) -> TardisEntitlement:
    return TardisEntitlement(
        coverage=coverage(*data_types),
        allowed_data_types=frozenset(data_types),
        sample_only=sample_only,
    )


def request(data_type: TardisDataType, start_day: int = 0, day_count: int = 1) -> FetchRequest:
    start = date(2020, 1, 1).toordinal() - date(1970, 1, 1).toordinal() + start_day
    start_ns = start * DAY_NS
    return FetchRequest(
        provider="tardis",
        dataset=data_type.value,
        instrument_id="DERIBIT:BTC-PERPETUAL",
        start_ns=start_ns,
        end_ns=start_ns + day_count * DAY_NS,
    )


def csv_gzip(header: str, *rows: str) -> bytes:
    text = "\n".join((header, *rows, ""))
    return gzip.compress(text.encode("utf-8"), mtime=0)


def trade_csv(price: str = "6425.50", amount: str = "1.250") -> bytes:
    return csv_gzip(
        "exchange,symbol,timestamp,local_timestamp,id,side,price,amount",
        f"deribit,BTC-PERPETUAL,1577836800000000,1577836800000100,t1,buy,{price},{amount}",
    )


def test_exchange_metadata_drives_interval_capabilities_conservatively() -> None:
    document: dict[str, object] = {
        "datasets": {
            "exportedUntil": "2020-01-31",
            "symbols": [
                {
                    "id": "BTC-PERPETUAL",
                    "availableSince": "2020-01-01",
                    "availableTo": "2020-01-31",
                    "dataTypes": [
                        "trades",
                        "quotes",
                        "incremental_book_L2",
                        "derivative_ticker",
                        "liquidations",
                        "options_chain",
                    ],
                }
            ],
        }
    }
    parsed = TardisCoverage.from_exchange_details(
        document,
        exchange="deribit",
        symbol="BTC-PERPETUAL",
    )
    policy = TardisEntitlement(parsed, parsed.data_types, sample_only=False)
    caps = capabilities_for_interval(
        policy,
        start_day=date(2020, 1, 1),
        end_day=date(2020, 1, 2),
    )
    assert caps.execution_tier == "F2"
    assert caps.has_l2_snapshots and caps.has_l2_deltas
    assert caps.has_trades and caps.has_bbo
    assert caps.has_funding and caps.has_liquidations


def test_sample_entitlement_fails_before_non_sample_day_is_planned() -> None:
    adapter = TardisAdapter(
        config(TardisDataType.TRADES),
        entitlement(TardisDataType.TRADES, sample_only=True),
        transport=FakeTransport({}),
    )
    first = adapter.plan(request(TardisDataType.TRADES))
    assert len(first.chunks) == 1
    assert first.chunks[0].source_ref.endswith("/2020/01/01/BTC-PERPETUAL.csv.gz")
    with pytest.raises(TardisRequestError, match="entitlement"):
        adapter.plan(request(TardisDataType.TRADES, start_day=1))


def test_authenticated_fetch_keeps_secret_out_of_plan_and_source_ref() -> None:
    cfg = config(TardisDataType.TRADES)
    policy = entitlement(TardisDataType.TRADES)
    provisional = TardisAdapter(cfg, policy, transport=FakeTransport({}), api_key_provider=lambda: "secret")
    chunk = provisional.plan(request(TardisDataType.TRADES)).chunks[0]
    transport = FakeTransport({chunk.source_ref: trade_csv()})
    adapter = TardisAdapter(cfg, policy, transport=transport, api_key_provider=lambda: "secret")
    raw = adapter.fetch(chunk)
    assert raw == transport.responses[chunk.source_ref]
    assert "secret" not in chunk.source_ref and "secret" not in chunk.key
    assert transport.calls == [(chunk.source_ref, "secret")]


def test_trade_normalization_is_exact_and_uses_row_position_tie_breaker() -> None:
    cfg = config(TardisDataType.TRADES)
    policy = entitlement(TardisDataType.TRADES)
    adapter = TardisAdapter(cfg, policy, transport=FakeTransport({}), api_key_provider=lambda: "secret")
    chunk = adapter.plan(request(TardisDataType.TRADES)).chunks[0]
    event = adapter.normalize(chunk, trade_csv()).events[0]
    assert event["ts_event_ns"] == "1577836800000000000"
    assert event["ts_recv_ns"] == "1577836800000100000"
    assert event["canonical_tie_breaker"] == "0"
    assert event["source_event_id"] == "t1"
    assert event["payload"] == {
        "price_atoms": "642550",
        "qty_atoms": "1250",
        "aggressor_side": "BUY",
        "trade_id": "t1",
    }
    with pytest.raises(TardisSchemaError, match="precision"):
        adapter.normalize(chunk, trade_csv(price="6425.501"))


def test_incremental_l2_skips_buffered_rows_and_marks_reconnect_snapshot() -> None:
    raw = csv_gzip(
        "exchange,symbol,timestamp,local_timestamp,is_snapshot,side,price,amount",
        "deribit,BTC-PERPETUAL,1577836800000000,1577836800000010,false,bid,100.00,1.000",
        "deribit,BTC-PERPETUAL,1577836800000020,1577836800000030,true,bid,100.00,2.000",
        "deribit,BTC-PERPETUAL,1577836800000020,1577836800000030,true,ask,101.00,3.000",
        "deribit,BTC-PERPETUAL,1577836800000040,1577836800000050,false,bid,100.00,1.500",
        "deribit,BTC-PERPETUAL,1577836800000060,1577836800000070,true,bid,99.50,4.000",
        "deribit,BTC-PERPETUAL,1577836800000060,1577836800000070,true,ask,101.50,5.000",
    )
    cfg = config(TardisDataType.INCREMENTAL_BOOK_L2)
    adapter = TardisAdapter(
        cfg,
        entitlement(TardisDataType.INCREMENTAL_BOOK_L2),
        transport=FakeTransport({}),
        api_key_provider=lambda: "secret",
    )
    chunk = adapter.plan(request(TardisDataType.INCREMENTAL_BOOK_L2)).chunks[0]
    events = adapter.normalize(chunk, raw).events
    assert [event["kind"] for event in events] == [
        "BOOK_SNAPSHOT_L2",
        "BOOK_DELTA_L2",
        "BOOK_SNAPSHOT_L2",
    ]
    assert events[0]["quality_flags"] == ["PRE_SNAPSHOT_UPDATES_SKIPPED"]
    assert events[1]["payload"] == {
        "side": "BUY",
        "price_atoms": "10000",
        "new_qty_atoms": "1500",
        "action": "UPSERT",
    }
    assert events[2]["quality_flags"] == ["PRE_SNAPSHOT_UPDATES_SKIPPED", "RECONNECT_SNAPSHOT"]


def test_derivative_ticker_emits_signed_funding_and_payment_time() -> None:
    raw = csv_gzip(
        "exchange,symbol,timestamp,local_timestamp,funding_timestamp,funding_rate,predicted_funding_rate,open_interest,last_price,index_price,mark_price",
        "deribit,BTC-PERPETUAL,1577836800000000,1577836800000100,1577865600000000,-0.000125,,12.345,100.00,99.50,99.75",
    )
    cfg = config(TardisDataType.DERIVATIVE_TICKER)
    adapter = TardisAdapter(
        cfg,
        entitlement(TardisDataType.DERIVATIVE_TICKER),
        transport=FakeTransport({}),
        api_key_provider=lambda: "secret",
    )
    chunk = adapter.plan(request(TardisDataType.DERIVATIVE_TICKER)).chunks[0]
    events = adapter.normalize(chunk, raw).events
    by_kind = {str(event["kind"]): event for event in events}
    assert by_kind["FUNDING_RATE"]["payload"] == {"value_atoms": "-125000", "unit": "rate"}
    assert by_kind["OPEN_INTEREST"]["payload"] == {"value_atoms": "12345", "unit": "quantity"}
    assert by_kind["INDEX_PRICE"]["payload"] == {"value_atoms": "9950", "unit": "price"}
    assert by_kind["MARK_PRICE"]["payload"] == {"value_atoms": "9975", "unit": "price"}
    assert by_kind["FUNDING_PAYMENT_TIME"]["payload"] == {
        "value_atoms": "1577865600000000000",
        "unit": "ns",
    }


def test_top_five_snapshot_and_liquidation_normalize_to_canonical_events() -> None:
    snapshot_header = ["exchange", "symbol", "timestamp", "local_timestamp"]
    snapshot_values = ["deribit", "BTC-PERPETUAL", "1577836800000000", "1577836800000100"]
    for level in range(5):
        snapshot_header.extend(
            [
                f"asks[{level}].price",
                f"asks[{level}].amount",
                f"bids[{level}].price",
                f"bids[{level}].amount",
            ]
        )
        if level == 0:
            snapshot_values.extend(["101.00", "2.000", "100.00", "3.000"])
        else:
            snapshot_values.extend(["", "", "", ""])
    raw_snapshot = csv_gzip(",".join(snapshot_header), ",".join(snapshot_values))
    cfg = config(TardisDataType.BOOK_SNAPSHOT_5)
    adapter = TardisAdapter(
        cfg,
        entitlement(TardisDataType.BOOK_SNAPSHOT_5),
        transport=FakeTransport({}),
        api_key_provider=lambda: "secret",
    )
    chunk = adapter.plan(request(TardisDataType.BOOK_SNAPSHOT_5)).chunks[0]
    snapshot_event = adapter.normalize(chunk, raw_snapshot).events[0]
    assert snapshot_event["payload"] == {
        "bids": [{"price_atoms": "10000", "qty_atoms": "3000"}],
        "asks": [{"price_atoms": "10100", "qty_atoms": "2000"}],
        "scope": "TOP_N",
        "depth": 5,
    }

    raw_liquidation = csv_gzip(
        "exchange,symbol,timestamp,local_timestamp,id,side,price,amount",
        "deribit,BTC-PERPETUAL,1577836800000000,1577836800000100,liq-1,sell,98.25,0.500",
    )
    liq_cfg = config(TardisDataType.LIQUIDATIONS)
    liq_adapter = TardisAdapter(
        liq_cfg,
        entitlement(TardisDataType.LIQUIDATIONS),
        transport=FakeTransport({}),
        api_key_provider=lambda: "secret",
    )
    liq_chunk = liq_adapter.plan(request(TardisDataType.LIQUIDATIONS)).chunks[0]
    liquidation = liq_adapter.normalize(liq_chunk, raw_liquidation).events[0]
    assert liquidation["kind"] == "LIQUIDATION_PRINT"
    assert liquidation["payload"] == {
        "price_atoms": "9825",
        "qty_atoms": "500",
        "side": "SELL",
        "liquidation_id": "liq-1",
    }


def test_two_day_runner_resumes_without_refetching_completed_day(tmp_path: Path) -> None:
    cfg = config(TardisDataType.TRADES)
    policy = entitlement(TardisDataType.TRADES)
    planning = TardisAdapter(cfg, policy, transport=FakeTransport({}), api_key_provider=lambda: "secret")
    plan = planning.plan(request(TardisDataType.TRADES, day_count=2))
    first, second = plan.chunks
    responses = {first.source_ref: trade_csv("100.00", "1.000"), second.source_ref: trade_csv("101.00", "2.000")}
    transport = FakeTransport(responses, fail_once_on=second.source_ref)
    adapter = TardisAdapter(cfg, policy, transport=transport, api_key_provider=lambda: "secret")

    def build_runner() -> IngestionRunner:
        return IngestionRunner(
            cache=ContentAddressedCache(tmp_path / "cache"),
            state=JobStateStore(tmp_path / "state"),
            budget=BudgetGuard(max_requests=10, max_bytes=10_000, max_cost_minor=100),
        )

    req = request(TardisDataType.TRADES, day_count=2)
    with pytest.raises(RuntimeError, match="synthetic Tardis interruption"):
        build_runner().run(
            adapter,
            req,
            output_path=tmp_path / "events.jsonl",
            manifest_path=tmp_path / "manifest.json",
        )
    build_runner().run(
        adapter,
        req,
        output_path=tmp_path / "events.jsonl",
        manifest_path=tmp_path / "manifest.json",
    )
    urls = [url for url, _ in transport.calls]
    assert urls.count(first.source_ref) == 1
    assert urls.count(second.source_ref) == 2
    assert b'"price_atoms":"10000"' in (tmp_path / "events.jsonl").read_bytes()
    assert b'"price_atoms":"10100"' in (tmp_path / "events.jsonl").read_bytes()
