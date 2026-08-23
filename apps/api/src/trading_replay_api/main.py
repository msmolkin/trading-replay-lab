"""Minimal FastAPI application used to prove the workspace scaffold."""

from fastapi import FastAPI

app = FastAPI(title="Trading Replay Lab API")


@app.get("/health")
def health() -> dict[str, str]:
    """Return a dependency-free liveness response."""
    return {"status": "ok"}


def run() -> None:
    """Run the development API server."""
    import uvicorn

    uvicorn.run("trading_replay_api.main:app", host="127.0.0.1", port=8000, reload=False)
