"use client";

import { useCallback, useEffect, useMemo, useState } from "react";

import type { OrderIntent, PracticeSnapshot, PriceMode, SubmitPracticeOrder } from "./types";

const INTENTS: readonly OrderIntent[] = ["BUY", "SELL", "SHORT", "COVER", "REVERSE", "CLOSE"];
const PRICE_MODES: readonly PriceMode[] = ["MARKET", "BID", "ASK", "MID", "STOP"];

export function PracticeGame() {
  const [game, setGame] = useState<PracticeSnapshot | null>(null);
  const [intent, setIntent] = useState<OrderIntent>("BUY");
  const [priceMode, setPriceMode] = useState<PriceMode>("MARKET");
  const [quantity, setQuantity] = useState("0.100");
  const [stopPrice, setStopPrice] = useState("64000");
  const [reduceOnly, setReduceOnly] = useState(false);
  const [postOnly, setPostOnly] = useState(false);
  const [marketableOnly, setMarketableOnly] = useState(true);
  const [leverage, setLeverage] = useState(5);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [shared, setShared] = useState(false);

  const start = useCallback(async () => {
    setBusy(true);
    setError(null);
    setPlaying(false);
    try {
      const next = await api<PracticeSnapshot>("/api/practice", { method: "POST" });
      setGame(next);
      setLeverage(next.leverage);
    } catch (cause) {
      setError(messageOf(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  const advance = useCallback(
    async (steps = 1) => {
      if (game?.status !== "ACTIVE") {
        return;
      }
      setBusy(true);
      setError(null);
      try {
        const next = await api<PracticeSnapshot>(`/api/practice/${game.sessionId}/advance`, {
          method: "POST",
          body: JSON.stringify({ steps }),
        });
        setGame(next);
        if (next.status !== "ACTIVE") {
          setPlaying(false);
        }
      } catch (cause) {
        setPlaying(false);
        setError(messageOf(cause));
      } finally {
        setBusy(false);
      }
    },
    [game],
  );

  useEffect(() => {
    if (!playing || !game || game.status !== "ACTIVE" || busy) {
      return;
    }
    const timer = window.setTimeout(() => void advance(speed), 850);
    return () => window.clearTimeout(timer);
  }, [advance, busy, game, playing, speed]);

  async function submitOrder() {
    if (!game) {
      return;
    }
    const quantityMilli = Math.round(Number(quantity) * 1_000);
    const request: SubmitPracticeOrder = {
      intent,
      quantityMilli,
      priceMode,
      stopPriceCents: priceMode === "STOP" ? Math.round(Number(stopPrice) * 100) : undefined,
      reduceOnly,
      postOnly,
      marketableOnly,
    };
    setBusy(true);
    setError(null);
    try {
      const next = await api<PracticeSnapshot>(`/api/practice/${game.sessionId}/order`, {
        method: "POST",
        body: JSON.stringify(request),
      });
      setGame(next);
    } catch (cause) {
      setError(messageOf(cause));
    } finally {
      setBusy(false);
    }
  }

  async function applyLeverage() {
    if (!game) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setGame(
        await api<PracticeSnapshot>(`/api/practice/${game.sessionId}/leverage`, {
          method: "POST",
          body: JSON.stringify({ leverage }),
        }),
      );
    } catch (cause) {
      setLeverage(game.leverage);
      setError(messageOf(cause));
    } finally {
      setBusy(false);
    }
  }

  async function shareResult() {
    if (!game) {
      return;
    }
    const text = `Trading Replay Lab fixture: ${formatMoney(game.equityCents)} equity after ${game.step} steps.`;
    if (navigator.share) {
      try {
        await navigator.share({
          title: "Trading Replay Lab result",
          text,
          url: window.location.href,
        });
        return;
      } catch {
        return;
      }
    }
    await navigator.clipboard.writeText(`${text} ${window.location.href}`);
    setShared(true);
    window.setTimeout(() => setShared(false), 1800);
  }

  if (!game) {
    return (
      <main className="landing-shell">
        <section className="landing-card">
          <div className="eyebrow">Deterministic market replay</div>
          <h1>Can your trading survive the path?</h1>
          <p>
            Trade an unrevealed BTC path with leverage, reversals, maker/taker constraints, stops,
            and reduce-only protection. Every click moves through the same fixed fixture.
          </p>
          <button className="primary-action" type="button" onClick={start} disabled={busy}>
            {busy ? "Opening market…" : "Start practice run"}
          </button>
          <p className="disclosure">
            Synthetic F1 practice data · $10,000 simulated equity · no account or real money
          </p>
          {error ? <p className="error-banner">{error}</p> : null}
        </section>
      </main>
    );
  }

  const returnCents = game.equityCents - game.startingEquityCents;
  const positionLabel =
    game.positionMilli === 0
      ? "Flat"
      : `${game.positionMilli > 0 ? "Long" : "Short"} ${formatQuantity(Math.abs(game.positionMilli))}`;

  return (
    <main className="game-shell">
      <header className="topbar">
        <div>
          <div className="eyebrow">Trading Replay Lab</div>
          <h1>SYNTH-BTC-USD</h1>
        </div>
        <div className="status-cluster" role="status" aria-label="Session status">
          <span className="badge badge--fidelity">{game.fidelity}</span>
          <span className={`badge badge--${game.status.toLowerCase()}`}>{game.status}</span>
        </div>
      </header>

      <section className="ticker-strip" aria-label="Visible market quote">
        <Metric label="Bid" value={formatMoney(game.bidPriceCents)} />
        <Metric label="Last" value={formatMoney(game.markPriceCents)} strong />
        <Metric label="Ask" value={formatMoney(game.askPriceCents)} />
        <Metric label="Elapsed" value={`${game.step} / ${game.totalSteps}`} />
      </section>

      <section className="workspace">
        <div className="market-column">
          <section className="panel chart-panel" aria-labelledby="chart-title">
            <div className="panel-heading">
              <div>
                <h2 id="chart-title">Revealed path</h2>
                <p>Future prints remain on the server until you advance.</p>
              </div>
              <span className={returnCents >= 0 ? "positive" : "negative"}>
                {signedMoney(returnCents)}
              </span>
            </div>
            <PriceChart game={game} />
            <fieldset className="playback-controls">
              <legend className="sr-only">Replay controls</legend>
              <button
                type="button"
                className="control-button"
                onClick={() => setPlaying((value) => !value)}
                disabled={game.status !== "ACTIVE"}
              >
                {playing ? "Pause" : "Play"}
              </button>
              <button
                type="button"
                className="control-button"
                onClick={() => void advance(1)}
                disabled={busy || game.status !== "ACTIVE"}
              >
                Step
              </button>
              <label>
                Speed
                <select
                  value={speed}
                  onChange={(event) => setSpeed(Number(event.currentTarget.value))}
                >
                  <option value={1}>1×</option>
                  <option value={2}>2×</option>
                  <option value={4}>4×</option>
                </select>
              </label>
              <progress value={game.step} max={game.totalSteps} aria-label="Episode progress" />
            </fieldset>
          </section>

          <section className="panel account-panel" aria-labelledby="account-title">
            <div className="panel-heading">
              <div>
                <h2 id="account-title">Account</h2>
                <p>{positionLabel}</p>
              </div>
              <label className="leverage-control">
                Leverage
                <span>
                  <input
                    aria-label="Leverage"
                    type="number"
                    min={1}
                    max={50}
                    step={1}
                    value={leverage}
                    onChange={(event) => setLeverage(Number(event.currentTarget.value))}
                  />
                  <button
                    type="button"
                    onClick={applyLeverage}
                    disabled={busy || leverage === game.leverage}
                  >
                    Apply
                  </button>
                </span>
              </label>
            </div>
            <div className="metric-grid">
              <Metric label="Equity" value={formatMoney(game.equityCents)} strong />
              <Metric label="Unrealized P&L" value={signedMoney(game.unrealizedPnlCents)} />
              <Metric label="Realized P&L" value={signedMoney(game.realizedPnlCents)} />
              <Metric label="Fees" value={`−${formatMoney(game.feesCents)}`} />
              <Metric label="Avg entry" value={maybeMoney(game.averageEntryCents)} />
              <Metric
                label="Effective leverage"
                value={`${(game.effectiveLeverageMilli / 1_000).toFixed(2)}×`}
              />
              <Metric label="Initial margin" value={formatMoney(game.initialMarginCents)} />
              <Metric label="Available margin" value={formatMoney(game.availableMarginCents)} />
              <Metric label="Liquidation buffer" value={formatMoney(game.liquidationBufferCents)} />
              <Metric label="Max drawdown" value={formatMoney(game.maxDrawdownCents)} />
            </div>
          </section>
        </div>

        <aside className="trade-column">
          <section className="panel ticket" aria-labelledby="ticket-title">
            <div className="panel-heading">
              <div>
                <h2 id="ticket-title">Order ticket</h2>
                <p>One order can cross through zero unless reduce-only is enabled.</p>
              </div>
            </div>
            <fieldset className="intent-grid">
              <legend className="sr-only">Order intent</legend>
              {INTENTS.map((value) => (
                <button
                  key={value}
                  type="button"
                  className={
                    intent === value ? "intent-button intent-button--active" : "intent-button"
                  }
                  aria-pressed={intent === value}
                  onClick={() => setIntent(value)}
                >
                  {intentLabel(value)}
                </button>
              ))}
            </fieldset>
            <div className="ticket-fields">
              <label>
                Quantity (BTC)
                <input
                  type="number"
                  min="0.001"
                  max="100"
                  step="0.001"
                  value={quantity}
                  disabled={intent === "CLOSE"}
                  onChange={(event) => setQuantity(event.currentTarget.value)}
                />
              </label>
              <label>
                Price instruction
                <select
                  value={priceMode}
                  onChange={(event) => {
                    const next = event.currentTarget.value as PriceMode;
                    setPriceMode(next);
                    if (next === "MARKET" || next === "STOP") {
                      setMarketableOnly(true);
                      setPostOnly(false);
                    }
                  }}
                >
                  {PRICE_MODES.map((value) => (
                    <option key={value} value={value}>
                      {priceModeLabel(value)}
                    </option>
                  ))}
                </select>
              </label>
              {priceMode === "STOP" ? (
                <label>
                  Stop price (USD)
                  <input
                    type="number"
                    min="1"
                    step="0.01"
                    value={stopPrice}
                    onChange={(event) => setStopPrice(event.currentTarget.value)}
                  />
                </label>
              ) : null}
            </div>
            <div className="order-flags">
              <label>
                <input
                  type="checkbox"
                  checked={reduceOnly}
                  onChange={(event) => setReduceOnly(event.currentTarget.checked)}
                />
                Reduce-only
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={postOnly}
                  disabled={priceMode === "MARKET" || priceMode === "STOP"}
                  onChange={(event) => {
                    setPostOnly(event.currentTarget.checked);
                    if (event.currentTarget.checked) setMarketableOnly(false);
                  }}
                />
                Maker-only
              </label>
              <label>
                <input
                  type="checkbox"
                  checked={marketableOnly}
                  disabled={priceMode === "STOP"}
                  onChange={(event) => {
                    setMarketableOnly(event.currentTarget.checked);
                    if (event.currentTarget.checked) setPostOnly(false);
                  }}
                />
                Taker-only
              </label>
            </div>
            <button
              className={
                intent === "SELL" || intent === "SHORT"
                  ? "submit-order submit-order--sell"
                  : "submit-order"
              }
              type="button"
              onClick={submitOrder}
              disabled={busy || game.status !== "ACTIVE"}
            >
              {busy
                ? "Processing…"
                : `${intentLabel(intent)} ${intent === "CLOSE" ? "position" : `${quantity} BTC`}`}
            </button>
            {error ? (
              <p className="error-banner" role="alert">
                {error}
              </p>
            ) : null}
          </section>

          <section className="panel orders-panel" aria-labelledby="orders-title">
            <div className="panel-heading">
              <div>
                <h2 id="orders-title">Order activity</h2>
                <p>
                  {game.orders.length === 0 ? "No orders yet." : `${game.orders.length} recorded`}
                </p>
              </div>
            </div>
            <div className="order-list">
              {game.orders.slice(0, 8).map((order) => (
                <article className="order-row" key={order.id}>
                  <div>
                    <strong>{intentLabel(order.intent)}</strong>
                    <span>{formatQuantity(order.filledMilli || order.quantityMilli)}</span>
                  </div>
                  <div>
                    <span className={`order-status order-status--${order.status.toLowerCase()}`}>
                      {order.status}
                    </span>
                    <span>
                      {order.priceCents === null ? "Pending" : formatMoney(order.priceCents)}
                    </span>
                  </div>
                  <p>{order.message}</p>
                </article>
              ))}
            </div>
          </section>
        </aside>
      </section>

      {game.status !== "ACTIVE" ? (
        <section className="result-panel" aria-labelledby="result-title">
          <div className="eyebrow">Run complete</div>
          <h2 id="result-title">You finished with {formatMoney(game.equityCents)}</h2>
          <p>
            Net result {signedMoney(returnCents)} · maximum drawdown{" "}
            {formatMoney(game.maxDrawdownCents)} ·{" "}
            {game.status === "LIQUIDATED" ? "liquidated" : "survived"}.
          </p>
          <div className="result-actions">
            <button type="button" className="primary-action" onClick={start}>
              Play again
            </button>
            <button type="button" className="control-button" onClick={shareResult}>
              {shared ? "Copied!" : "Share result"}
            </button>
          </div>
          <p className="disclosure">
            Fixture result only. Fills use simulated F1 quotes and do not predict live performance.
          </p>
        </section>
      ) : null}
      <footer>
        Synthetic practice fixture · deterministic path · future prices withheld until reveal · not
        financial advice
      </footer>
    </main>
  );
}

function PriceChart({ game }: { game: PracticeSnapshot }) {
  const width = 900;
  const height = 330;
  const padding = 24;
  const points = useMemo(() => {
    const values = game.revealed.map((point) => point.priceCents);
    const minimum = Math.min(...values);
    const maximum = Math.max(...values);
    const spread = Math.max(1, maximum - minimum);
    return game.revealed
      .map((point, index) => {
        const x = padding + (index / Math.max(1, game.totalSteps)) * (width - padding * 2);
        const y =
          height - padding - ((point.priceCents - minimum) / spread) * (height - padding * 2);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" ");
  }, [game]);
  return (
    <svg
      className="price-chart"
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={`Revealed price chart through step ${game.step}; last price ${formatMoney(game.markPriceCents)}`}
    >
      <defs>
        <linearGradient id="chart-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="#4f8cff" stopOpacity="0.35" />
          <stop offset="1" stopColor="#4f8cff" stopOpacity="0" />
        </linearGradient>
      </defs>
      <g className="chart-grid">
        {[0, 1, 2, 3].map((line) => (
          <line
            key={line}
            x1={padding}
            x2={width - padding}
            y1={padding + line * 90}
            y2={padding + line * 90}
          />
        ))}
      </g>
      {game.revealed.length > 1 ? (
        <>
          <polygon
            points={`${padding},${height - padding} ${points} ${points.split(" ").at(-1)?.split(",")[0]},${height - padding}`}
            fill="url(#chart-fill)"
          />
          <polyline
            points={points}
            fill="none"
            stroke="#65a0ff"
            strokeWidth="4"
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        </>
      ) : (
        <circle cx={padding} cy={height / 2} r="5" fill="#65a0ff" />
      )}
    </svg>
  );
}

function Metric({
  label,
  value,
  strong = false,
}: {
  label: string;
  value: string;
  strong?: boolean;
}) {
  return (
    <div className="metric">
      <span>{label}</span>
      <strong className={strong ? "metric--strong" : undefined}>{value}</strong>
    </div>
  );
}

async function api<T>(path: string, init: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: { "content-type": "application/json", ...init.headers },
  });
  const body: unknown = await response.json();
  if (!response.ok) {
    const detail =
      typeof body === "object" && body !== null && "error" in body && typeof body.error === "string"
        ? body.error
        : `Request failed (${response.status}).`;
    throw new Error(detail);
  }
  return body as T;
}

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : "The practice session could not continue.";
}

function formatMoney(cents: number): string {
  return new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(cents / 100);
}

function maybeMoney(cents: number | null): string {
  return cents === null ? "—" : formatMoney(cents);
}

function signedMoney(cents: number): string {
  return `${cents >= 0 ? "+" : "−"}${formatMoney(Math.abs(cents))}`;
}

function formatQuantity(milli: number): string {
  return `${(milli / 1_000).toFixed(3)} BTC`;
}

function intentLabel(intent: OrderIntent): string {
  return (
    {
      BUY: "Buy / long",
      SELL: "Sell",
      SHORT: "Sell short",
      COVER: "Cover",
      REVERSE: "Reverse",
      CLOSE: "Close",
    } as const
  )[intent];
}

function priceModeLabel(mode: PriceMode): string {
  return (
    {
      MARKET: "Market",
      BID: "At bid",
      ASK: "At ask",
      MID: "At midpoint",
      STOP: "Stop market",
    } as const
  )[mode];
}
