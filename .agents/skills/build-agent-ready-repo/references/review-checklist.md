# Review and publication checklist

Apply this after files are written. Use the repository's native tools where available; do not add heavyweight dependencies solely for this audit.

## Intent and semantics

- Every explicit user capability appears in the normative documents.
- Ambiguous verbs have one documented canonical meaning and edge-case examples.
- Goals, non-goals, assumptions, deferred decisions, and disclaimers are distinguishable.
- External/provider claims have current primary sources or are labeled proposals.
- No document promises precision, coverage, security, or scale that the design cannot support.

## Agent operability

- Root `AGENTS.md` is self-contained for a cloud agent.
- Required reading links resolve; archives/research dumps are correctly classified.
- Authority boundaries prevent domain logic from being independently reimplemented.
- Every task has ID, dependency, owned paths, deliverable, and acceptance evidence.
- Tasks marked ready have no unmet dependency.
- Parallel ready tasks do not overlap paths or require unresolved shared decisions.
- The first task from a clean clone is obvious.

## Contracts and tests

- Machine-readable schemas parse and representative examples validate.
- Exact values have declared integer/fixed-point units and timestamp semantics.
- Compatibility and unknown-value behavior are explicit.
- Golden scenarios cover crossings, partial operations, ordering ties, retries, failure/recovery, and numeric boundaries as applicable.
- Security boundaries receive integration/adversarial tests, not only unit tests.
- Determinism or reproducibility claims have a hash/version/fixture strategy.

## Repository safety

- Inspect `git status`, intended diffs, ignored files, and existing user changes.
- Run whitespace/format checks and local Markdown-link validation.
- Scan for common secret formats, private keys, signed URLs, credentials, and unexpectedly large/binary files.
- Confirm archives contain only material the user asked to preserve.
- Confirm no raw licensed/private data is staged.
- Run relevant schema parsers and repository tests.

Record commands and results. A passing grep is supporting evidence, not proof of security.

## Git and remote

- Commit author and branch are appropriate.
- Default branch is explicit.
- Repository owner/name/visibility match the request.
- Remote description/topics aid discovery without overstating status.
- Local HEAD equals remote default-branch commit after push.
- Remote API reports expected public/private visibility.
- The initial ready issue links the task and agent instructions if issue creation is in scope.

## Handoff completeness

Report:

- public/private repository URL and local path;
- key starting documents and current ready task;
- commit ID and validation summary;
- external actions performed;
- unsupported/deferred areas and any blocked publication step.

Do not call the work complete if the user requested publication and only a local repository exists.
