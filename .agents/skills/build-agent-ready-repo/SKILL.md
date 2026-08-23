---
name: build-agent-ready-repo
description: Turn a rough software, game, or product idea into a decision-complete repository that many coding agents can implement in parallel. Use when the user asks for an implementation-ready spec, agent-ready repository, task graph, standalone AGENTS.md, or public project scaffold; do not use for ordinary feature implementation or brainstorming that is not meant to produce a repository.
---

# Build an Agent-Ready Repository

Produce a repository in which independent coding agents can discover the intent, claim non-overlapping work, implement against stable contracts, and verify completion without inventing product semantics.

## Preserve the assignment

- Respect the user's requested depth, technologies, destination, visibility, license, and publishing scope.
- Inspect the workspace before choosing a directory or modifying an existing repository.
- Ask only when a missing choice would materially change the product or authorize a significant external action. Make and record low-risk assumptions otherwise.
- Treat publishing, changing repository visibility, creating many external issues, or configuring paid services as separate actions requiring user authorization. An instruction to make a repository public is sufficient authorization to create and push that repository, but not to enable paid services.
- If asked only to design/specify, do not begin product implementation.
- Preserve a raw request only when the user asks. Put it in an archive clearly excluded from agent routing; current specifications remain authoritative.

## Resolve ambiguity before parallelizing

Turn feature language into normative behavior. Define edge cases, precedence, failure behavior, and what is intentionally deferred. For any stateful or quantitative system, identify its source of truth, numeric units, state transitions, idempotency behavior, and invariants.

Research facts that are current, niche, provider-specific, regulated, or likely to be misremembered. Prefer official/primary sources, record the checked date, and separate confirmed capability from a proposed adapter or approximation.

For a full greenfield repository, read [references/repository-blueprint.md](references/repository-blueprint.md) before writing files. Adapt the blueprint to the domain; do not mechanically create every listed document.

## Make the repository agent-operable

- Put a self-contained `AGENTS.md` at the applicable repository root. Cloud agents may have no global instructions.
- State the mission, required reading, non-negotiable invariants, authority boundaries, testing expectations, secrets/data rules, work-claim protocol, and pull-request handoff.
- Keep archived prompts, research dumps, and examples out of required reading unless they are genuinely normative.
- Split implementation into stable task IDs with dependencies, exclusive owned paths, concrete deliverables, and observable acceptance checks.
- Make contract and bootstrap tasks precede their consumers. Parallel tasks must not need to edit the same files or independently decide the same semantics.
- Mark only dependency-satisfied tasks ready. Explain how an agent claims work and how cross-cutting changes are serialized.
- Seed machine-readable contracts or fixtures when they remove ambiguity, but do not disguise placeholder application code as implementation progress.

## Validate and publish

Before committing or publishing, read [references/review-checklist.md](references/review-checklist.md) and run the applicable checks. Fix failures rather than merely reporting them.

Preserve unrelated user changes. Keep secrets, licensed data, generated caches, and credentials out of Git. Confirm that the working tree contains only intended changes before committing.

When publication is authorized:

1. Confirm the intended owner/name is available and authentication can create the repository.
2. Create the requested visibility explicitly; never rely on a default.
3. Push the intended default branch and add a concise description/topics when useful for discovery.
4. Verify through the remote API that visibility and default branch are correct and that local/remote commit IDs match.
5. Create only the issue labels/issues that make the next work claimable. Avoid flooding a new repository with blocked issues unless requested.

## Handoff

Lead with the completed outcome. Provide the repository URL, local path, key entry documents, current ready task, validation evidence, and any honest limitation. If publishing was blocked, leave a clean local repository and state the exact missing authorization or credential condition.
