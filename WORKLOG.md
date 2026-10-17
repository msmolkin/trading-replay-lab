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
