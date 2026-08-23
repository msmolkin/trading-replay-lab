from __future__ import annotations

import hashlib
import io
import zipfile
from dataclasses import dataclass
from datetime import date

import pytest

from trading_replay_ingest.adapters.binance_public import (
    ArchiveRevisionDetected,
    BinanceDataKind,
    BinanceIntegrityError,
    BinanceProduct,
    BinancePublicAdapter,
    BinancePublicConfig,
    BinanceSchemaError,
)
from trading_replay_ingest.core import FetchRequest


@dataclass(slots=True)
class FakeTransport:
    objects: dict[str, bytes]

    def get(self, url: str, max_bytes: int) -> bytes:
        payload = self.objects[url]
        if len(payload) > max_bytes:
            raise AssertionError("test transport object exceeds requested maximum")
        return payload


def config(
    *,
    product: BinanceProduct = BinanceProduct.SPOT,
    kind: BinanceDataKind = BinanceDataKind.TRADES,
    interval: str | None = None,
) -> BinancePublicConfig:
    return BinancePublicConfig(
        product=product,
        data_kind=kind,
        symbol="BTCUSDT",
        instrument_id="BTC-USDT",
        dataset_id=f"binance-{product.value}-{kind.value}",
        price_scale=2,
        qty_scale=3,
        interval=interval,
        max_archive_bytes=1_000_000,
        max_uncompressed_bytes=1_000_000,
    )


def request(kind: BinanceDataKind, start_ns: int, end_ns: int) -> FetchRequest:
    return FetchRequest(
        provider="binance_public",
        dataset=kind.value,
        instrument_id="BTC-USDT",
        start_ns=start_ns,
        end_ns=end_ns,
    )


def zipped(filename: str, text: str, *, member_name: str | None = None) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(member_name or filename.removesuffix(".zip") + ".csv", text)
    return output.getvalue()


def transport_for(url: str, raw: bytes, *, digest: str | None = None) -> FakeTransport:
    checksum = hashlib.sha256(raw).hexdigest() if digest is None else digest
    filename = url.rsplit("/", maxsplit=1)[-1]
    return FakeTransport(
        {
            url: raw,
            f"{url}.CHECKSUM": f"{checksum}  {filename}\n".encode(),
        }
    )


def day_ns(value: date) -> int:
    return (value - date(1970, 1, 1)).days * 86_400_000_000_000


def test_plan_uses_documented_spot_and_futures_paths() -> None:
    target = date(2025, 1, 2)
    start = day_ns(target)
    end = start + 1

    spot = BinancePublicAdapter(config(), transport=FakeTransport({}))
    spot_chunk = spot.plan(request(BinanceDataKind.TRADES, start, end)).chunks[0]
    assert spot_chunk.source_ref.endswith(
        "/data/spot/daily/trades/BTCUSDT/BTCUSDT-trades-2025-01-02.zip"
    )

    usd_m = BinancePublicAdapter(
        config(product=BinanceProduct.USD_M, kind=BinanceDataKind.KLINES, interval="1m"),
        transport=FakeTransport({}),
    )
    futures_request = request(BinanceDataKind.KLINES, start, end)
    futures_chunk = usd_m.plan(futures_request).chunks[0]
    assert futures_chunk.source_ref.endswith(
        "/data/futures/um/daily/klines/BTCUSDT/1m/BTCUSDT-1m-2025-01-02.zip"
    )


def test_spot_timestamp_unit_switch_is_date_driven() -> None:
    before_day = date(2024, 12, 31)
    after_day = date(2025, 1, 1)
    before_timestamp_ms = "1735603200123"
    after_timestamp_us = "1735689600123000"

    adapter = BinancePublicAdapter(config(), transport=FakeTransport({}))
    before_url = adapter.archive_url(before_day)
    before_raw = zipped(
        before_url.rsplit("/", maxsplit=1)[-1],
        f"1,100.00,0.125,12.50,{before_timestamp_ms},false,true\n",
    )
    before_adapter = BinancePublicAdapter(config(), transport=transport_for(before_url, before_raw))
    before_chunk = before_adapter.plan(
        request(BinanceDataKind.TRADES, day_ns(before_day), day_ns(before_day) + 1)
    ).chunks[0]
    before_event = before_adapter.normalize(
        before_chunk, before_adapter.fetch(before_chunk)
    ).events[0]
    assert before_event["ts_event_ns"] == "1735603200123000000"

    after_url = adapter.archive_url(after_day)
    after_raw = zipped(
        after_url.rsplit("/", maxsplit=1)[-1],
        f"2,100.00,0.125,12.50,{after_timestamp_us},true,true\n",
    )
    after_adapter = BinancePublicAdapter(config(), transport=transport_for(after_url, after_raw))
    after_chunk = after_adapter.plan(
        request(BinanceDataKind.TRADES, day_ns(after_day), day_ns(after_day) + 1)
    ).chunks[0]
    after_event = after_adapter.normalize(after_chunk, after_adapter.fetch(after_chunk)).events[0]
    assert after_event["ts_event_ns"] == "1735689600123000000"
    assert after_event["payload"] == {
        "price_atoms": "10000",
        "qty_atoms": "125",
        "aggressor_side": "SELL",
        "trade_id": "2",
    }


def test_futures_remain_millisecond_based_and_never_claim_depth() -> None:
    target = date(2025, 1, 1)
    cfg = config(product=BinanceProduct.USD_M)
    shell = BinancePublicAdapter(cfg, transport=FakeTransport({}))
    url = shell.archive_url(target)
    raw = zipped(
        url.rsplit("/", maxsplit=1)[-1],
        "7,100.00,0.125,12.50,1735689600123,false\n",
    )
    adapter = BinancePublicAdapter(cfg, transport=transport_for(url, raw))
    chunk = adapter.plan(
        request(BinanceDataKind.TRADES, day_ns(target), day_ns(target) + 1)
    ).chunks[0]
    event = adapter.normalize(chunk, adapter.fetch(chunk)).events[0]
    assert event["ts_event_ns"] == "1735689600123000000"
    assert cfg.capabilities == ("F0T",)
    assert "BBO" not in cfg.capabilities
    assert "F2" not in cfg.capabilities


def test_kline_normalization_is_exact_f0() -> None:
    target = date(2025, 1, 1)
    cfg = config(kind=BinanceDataKind.KLINES, interval="1m")
    shell = BinancePublicAdapter(cfg, transport=FakeTransport({}))
    url = shell.archive_url(target)
    raw = zipped(
        url.rsplit("/", maxsplit=1)[-1],
        "1735689600000000,100.00,101.00,99.00,100.50,1.250,1735689659999999,0,42,0,0,0\n",
    )
    adapter = BinancePublicAdapter(cfg, transport=transport_for(url, raw))
    chunk = adapter.plan(
        request(BinanceDataKind.KLINES, day_ns(target), day_ns(target) + 1)
    ).chunks[0]
    event = adapter.normalize(chunk, adapter.fetch(chunk)).events[0]
    assert event["kind"] == "BAR"
    assert event["payload"] == {
        "interval": "1m",
        "open_atoms": "10000",
        "high_atoms": "10100",
        "low_atoms": "9900",
        "close_atoms": "10050",
        "base_volume_atoms": "1250",
        "trade_count": "42",
        "complete": True,
    }
    assert cfg.capabilities == ("F0",)


def test_checksum_mismatch_and_revision_are_explicit() -> None:
    target = date(2024, 1, 1)
    shell = BinancePublicAdapter(config(), transport=FakeTransport({}))
    url = shell.archive_url(target)
    raw = zipped(url.rsplit("/", maxsplit=1)[-1], "1,100.00,1.000,100,1,false,true\n")

    bad = BinancePublicAdapter(config(), transport=transport_for(url, raw, digest="0" * 64))
    chunk = bad.plan(request(BinanceDataKind.TRADES, day_ns(target), day_ns(target) + 1)).chunks[0]
    with pytest.raises(BinanceIntegrityError, match="SHA-256"):
        bad.fetch(chunk)

    digest = hashlib.sha256(raw).hexdigest()
    revised = BinancePublicAdapter(
        config(),
        transport=transport_for(url, raw),
        known_checksums={url: "1" * 64},
    )
    with pytest.raises(ArchiveRevisionDetected, match="revision"):
        revised.fetch(chunk)
    clean = BinancePublicAdapter(
        config(), transport=transport_for(url, raw), known_checksums={url: digest}
    )
    clean.fetch(chunk)
    assert clean.observed_checksum(url) == digest


def test_unsafe_archive_and_inexact_decimal_fail_closed() -> None:
    target = date(2024, 1, 1)
    shell = BinancePublicAdapter(config(), transport=FakeTransport({}))
    url = shell.archive_url(target)
    filename = url.rsplit("/", maxsplit=1)[-1]
    unsafe_raw = zipped(filename, "1,100.00,1.000,100,1,false,true\n", member_name="../escape.csv")
    unsafe = BinancePublicAdapter(config(), transport=transport_for(url, unsafe_raw))
    chunk = unsafe.plan(request(BinanceDataKind.TRADES, day_ns(target), day_ns(target) + 1)).chunks[
        0
    ]
    with pytest.raises(BinanceIntegrityError, match="unsafe"):
        unsafe.normalize(chunk, unsafe.fetch(chunk))

    inexact_raw = zipped(filename, "1,100.001,1.000,100,1,false,true\n")
    inexact = BinancePublicAdapter(config(), transport=transport_for(url, inexact_raw))
    with pytest.raises(BinanceSchemaError, match="malformed"):
        inexact.normalize(chunk, inexact.fetch(chunk))
