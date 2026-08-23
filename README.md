# Trading Replay Lab

Trading Replay Lab is a historical-market game for testing whether a trading process survives the path that actually occurred—not merely whether a hindsight thesis was directionally right.

The player enters a sealed historical episode, changes leverage from 1× to 50×, and submits long, sell, short, cover, reverse, reduce-only, post-only, marketable-only, limit, market, and stop orders. A deterministic replay engine applies the best execution model the available data can justify. The session records every decision, fill, fee, funding charge, margin change, liquidation, and information reveal.

This repository currently contains the implementation-ready specification and parallel work graph. It intentionally contains no licensed market data.

## Start here

- Product behavior and acceptance criteria: [docs/product-spec.md](docs/product-spec.md)
- Exact order, position, margin, and liquidation semantics: [docs/trading-rules.md](docs/trading-rules.md)
- Architecture and service boundaries: [docs/architecture.md](docs/architecture.md)
- Canonical event and data contracts: [docs/data-contracts.md](docs/data-contracts.md)
- Anti-lookahead and blind-start design: [docs/anti-lookahead.md](docs/anti-lookahead.md)
- Provider feasibility and fidelity: [docs/data-providers.md](docs/data-providers.md)
- Verification strategy: [docs/testing.md](docs/testing.md)
- Parallel implementation plan: [tasks/README.md](tasks/README.md)
- Agent operating instructions: [AGENTS.md](AGENTS.md)

## Product principles

1. No lookahead. The server never sends unrevealed events to a player client.
2. No false precision. A candle-only session cannot claim exact maker fills, spread, queue position, or intrabar liquidation ordering.
3. Deterministic replay. The same input events, ruleset, seed, and commands produce the same ledger hash.
4. Explicit assumptions. Every result exports its data provenance, simulator version, fee model, fill model, and uncertainty flags.
5. Signed positions. One order may close a position and open the opposite side; reduce-only is the explicit guard against this.
6. Venue-safe data. Users supply credentials and accept provider terms; raw licensed data is not committed or redistributed.

## Proposed stack

- Web: Next.js, React, TypeScript, TradingView Lightweight Charts
- API/control plane: FastAPI, Python, PostgreSQL
- Deterministic simulator: Rust, exposed through a versioned command/event boundary
- Ingestion: Python workers writing canonical Parquet partitions and manifests
- Local analytics: DuckDB; object storage in production; Redis only for ephemeral jobs
- Contracts: JSON Schema plus generated TypeScript/Python/Rust models
- Deployment: Docker Compose locally, container-compatible services in production

These choices are decisions for the first implementation, not invitations for each agent to select a different stack. Propose changes through an ADR.

## Development bootstrap

The repository pins Node, Python, and Rust in `.node-version`, `.python-version`, and `rust-toolchain.toml`. Install those exact language versions plus GNU Make; Node's Corepack installs the pinned pnpm release during bootstrap.

From a clean clone, the single bootstrap command is:

```bash
make bootstrap
```

It verifies the pinned language versions, installs JavaScript and Python dependencies, fetches the Rust workspace, then runs every empty-stack formatter check, linter, type check, and test. After bootstrap, rerun the complete suite with `make check`; use `make format` to apply formatting.

## Intended modes

- `practice`: timestamps and instrument identity are visible.
- `blind-window`: the player chooses from an allowed coarse chart, then finer data is sealed.
- `random-sealed`: the server samples an eligible episode after the setup is committed.
- `challenge`: a shareable ruleset and episode commitment with comparable scoring.

The game is simulation and research software, not a broker, exchange, or promise of future results.

## Status

Implementation is underway from Milestone 0. The repository bootstrap is the first dependency for contracts, CI, simulator, ingestion, API, and web work; follow [tasks/README.md](tasks/README.md) for the dependency graph.

## Reusable Codex skill

This repository also contains [`$build-agent-ready-repo`](.agents/skills/build-agent-ready-repo/SKILL.md), a reusable Codex skill for turning a rough product idea into a specification repository that many coding agents can implement in parallel. Codex discovers it automatically while working in this repository. Invoke it explicitly with:

```text
$build-agent-ready-repo Turn this idea into an implementation-ready public repository: ...
```

For use across unrelated local repositories, copy or symlink the skill directory into `$HOME/.agents/skills/`. The skill is instruction-only and does not grant itself permission to publish repositories or create paid resources.

## License

Code and original documentation are MIT-licensed. Third-party data remains subject to its provider and venue terms. See [LICENSE](LICENSE) and [docs/data-providers.md](docs/data-providers.md).
