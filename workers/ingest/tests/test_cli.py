from trading_replay_ingest.cli import message


def test_message() -> None:
    assert message() == "trading-replay-ingest bootstrap ready"
