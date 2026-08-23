from trading_replay_api.main import health


def test_health() -> None:
    assert health() == {"status": "ok"}
