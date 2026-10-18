# Worklog

## 2026-10-16 — Studio CI migration

Goal: move existing CI compute to the local Studio with an explicit hosted rollback.

Changed only runs-on selection to a run-specific self-hosted macOS ARM64 profile. Public fork pull requests retain the original hosted label. All five CI jobs and nightly full-stack checks, policy self-test, exact Python3.14.7/Node24.19.0/Rust1.98.0/pnpm11.22.0 pins, formatting/type/lint/test/contract checks and schedule remain. Existing extended-suite placeholder hooks are preserved. Root AGENTS and required README/task/testing/architecture/ADR reading completed; M0-03 workflow ownership task is done with no active owner recorded. Current preparation expressly performs no task-claim/issue mutations.

Validation: parsed before/after equality permits only runs-on changes; default/configured/hosted/fork JSON resolutions checked with a sample run ID. No builds, test execution, installs, commits, dispatches, deployments, AMO signing or API writes were performed during preparation. Actual runtime validation remains after central review/application.

Hosted rollback values (retain JSON quotation marks): STUDIO_CI_UBUNTU_LATEST = "ubuntu-latest".

Runtime prerequisites: Before native admission, preseed Python3.14.7 as a writable venv at the effective /Users/runner/hostedtoolcache/Python/3.14.7/arm64 plus arm64.complete (same external toolcache symlink pattern). Official uv0.13.1 has exact DarwinARM64 artifact; setup-python fallback on Mac uses sudo installer. setup-node/rustup self-provision exact Node24.19.0 and Rust1.98.0 ARM64 versions; maintain writable toolcache, HOME, CARGO_HOME, RUSTUP_HOME and setup-node prefix for corepack/pnpm.

## 2026-10-17 — Run existing CI on Studio Linux

Moved the six runner defaults in ordinary and nightly CI to the existing Studio Linux ARM64 profile. The original native run failed dependency-cache hashing through the workspace alias and a generated Node CLI guard under a physical path containing spaces. The Linux workspace avoids those path conditions without changing application code, generated contracts, dependency pins, cache inputs or checks. Hosted rollback remains `ubuntu-latest` through the same repository variable.

Verified parsed workflow structure differs only in `runs-on`; the existing 98-repository allowlist and 108 route variables are unchanged. Provisioned and executed pinned Python 3.14.7 as nonadministrator UID1001 in the guest; pip and the ARM64 tool-cache layout pass. The route variable and workflow source were read back exactly. Actual whole-workflow execution is pending and will be recorded separately; the failed native attempt remains historical evidence.

## 2026-10-17 — Playable game vertical slice

Goal: make the current repository genuinely playable through its visible web UI and verify a complete first-player journey rather than treating completed backend pieces as a game.

Source baseline: public `msmolkin/trading-replay-lab` at `8f57a41`; clean checkout in this directory. Environment is macOS 27 on the local Mac, with responsive browser verification planned at 1365×900 and 390×844. The repository currently renders only “Monorepo bootstrap is ready,” so the core journey is blocked before a player can see a chart or place an order.

Chosen journey: launch a deterministic fixture episode, buy/short or close/reverse using visible controls, change leverage within 1×–50×, advance/pause replay, observe equity/P&L/order history, finish the episode, and restart. Fixture evidence will be labeled separately from real historical/provider data.

Constraints/decisions: preserve fixed-point accounting and no-lookahead behavior; do not put authoritative matching/accounting logic in presentation components; keep the change as a focused local-play vertical slice using existing repository patterns and synthetic redistributable inputs. No paid provider, deployment, or real brokerage action is in scope.

Current state: repository instructions, README, relevant task graph, existing worklog, and global app user-testing guide read. Next: inspect simulator/API boundaries and run the current web build before selecting the smallest safe integration path.

Outcome: the game is now playable at `http://127.0.0.1:3000` through `corepack pnpm play`. That one command starts the authoritative Go practice engine on loopback and the Next.js player. A fresh active run is open in the in-app browser.

Implemented: responsive trading UI; server-withheld deterministic 48-print fixture; play/pause/step and 1×/2×/4× playback; buy, sell, short, cover, reverse, close, cross-through-zero, reduce-only, maker-only, taker-only, bid/ask/mid/market/stop; 1×–50× leverage; account, P&L, margin, drawdown, order activity, completion, replay, and result sharing. Added favicon/PWA assets, metadata, and a custom 404. The browser now calls a Go fixed-point simulator through a small Go HTTP process; presentation code does not own matching/accounting.

Architecture decision: ADR-0002 supersedes Rust for new simulator work. The pre-existing Rust implementation is 13,451 lines and remains read-only migration/conformance evidence; deleting it without parity would be a separate milestone. Active task paths and agent guidance now target Go. M0-05 records the remaining generated-Go-contract migration explicitly.

Adversarial review found and drove fixes for margin admission omitting unrealized loss, risk-reducing fills being rejected while under margin, near-boundary spread loss, reduce-only/COVER excess audit status, working orders surviving terminal states, STOP retaining a maker-only flag, unsafe JSON integers, slow request bodies holding the global simulator lock, and launcher child cleanup. Regression tests cover each simulator issue.

Verified: Go `go test -race ./...`, `go vet ./...`, repeated deterministic core tests, HTTP journey tests, all workspace web/contract/UI tests, TypeScript, Biome formatting/lint, Next production build, schema generation drift, compatibility, cross-language round trips, repository policy, and `git diff --check`. Browser journeys at 1365×900 and 390×844 exercised start, buy, leverage change, sell through zero, partial cover cancellation, playback completion, and restart; no console warnings/errors were observed. The launcher's shutdown trap was checked by stopping it and confirming ports 3000 and 8787 were released.

Known limits: the playable run is a disclosed synthetic F1 fixture, not real historical data. Sessions are in memory, not persisted or idempotent, and the fixed path is knowable after replay. Real provider adapters, blind/random episode selection, persistence, and higher-fidelity execution remain follow-up milestones. Local Node is 24.21.0 while the repository pins 24.19.0; checks passed with only the expected engine warning.

Current state: work is verified locally and the one-command development game is running. Next: commit, push the public repository, confirm remote visibility/head, and leave the fresh run open for Michael.
