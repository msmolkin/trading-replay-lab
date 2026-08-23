# Repository blueprint

Use this reference for a full greenfield specification repository or a substantial conversion of an idea document. Select only artifacts that reduce ambiguity for the actual project.

## 1. Establish the decision surface

Extract and resolve:

- product outcome and target user;
- primary workflow and modes;
- explicit goals and non-goals;
- entities, state, commands, events, and external systems;
- user-visible semantics for every requested verb/control;
- edge cases, ordering, partial success, retries, and failure behavior;
- quality attributes: correctness, security, privacy, determinism, performance, accessibility, and operability;
- data availability, licensing, cost, and fidelity limitations;
- decisions that are settled versus deferred.

Do not let task slicing start while multiple agents could reasonably implement incompatible meanings for a core behavior.

## 2. Choose an authority map

Name exactly one authority for each consequential behavior. A useful table has:

| Concern | Authority | Consumers | Forbidden duplication |
|---|---|---|---|
| Domain state transition | Core/domain module | API, UI, workers | UI-side reimplementation |
| Authentication/authorization | Server boundary | All clients | Client-only enforcement |
| External normalization | Adapter layer | Domain/data store | Provider objects in core |
| Public contracts | Schema source | Generated models | Hand-copied cross-language types |

Prefer a modular monolith or a small number of processes until scale or isolation requires more. Architectural boundaries exist to assign authority and parallel ownership, not to maximize service count.

## 3. Recommended documents

### Root README

State the outcome, current status, key links, principles, chosen stack, start point, license/data boundary, and important disclaimer. It is a map, not a duplicate specification.

### Product specification

Include problem, goals/non-goals, personas, flows, modes, controls, lifecycle, setup/options, output/reporting, functional acceptance criteria, success measures, and explicit open decisions.

### Normative domain rules

Define exact command/state semantics, validation order, state transitions, precedence at equal timestamps, numeric representation, partial operations, idempotency, accounting or conservation rules, failure codes, and examples for ambiguous crossings.

### Architecture

Include a small system diagram only if it clarifies relationships. Define repository layout, component ownership, command/event or request boundaries, storage, time/ordering, external adapter interface, failure recovery, deployment profiles, performance targets, and versioning.

### Data/contracts

Define canonical envelopes, IDs, timestamps/units, null/unknown semantics, capability discovery, provenance/manifests, compatibility rules, and one or more machine-readable schema seeds. Do not use binary floating point for exact accounting values.

### Security, privacy, and compliance

Document the threat/trust model, authorization points, secrets handling, user-data sensitivity, dependency/supply-chain baseline, external data rights, disclosures, and out-of-scope expansions that need a new review.

### Testing

List unit, property, golden, contract, integration, end-to-end, adversarial, recovery, and performance tests in proportion to the system. Name required golden scenarios and invariants rather than saying only “add tests.”

### ADRs

Create an ADR template and record only decisions that are cross-cutting and costly to reverse. An ADR states context, decision, consequences, alternatives, and verification.

## 4. Data and adapter systems

When a product depends on third-party data:

- distinguish catalog/discovery, download, normalization, validation, storage, and product consumption;
- model capabilities per resource and time interval, not merely per provider;
- never claim a precision/fidelity unsupported by source fields;
- preserve source identifiers, schema/adapter version, checksums, coverage, gaps, and terms classification;
- quarantine corrupt or discontinuous data rather than inventing continuity;
- keep credentials and raw licensed data out of Git and default CI;
- make cost/entitlement checks occur before expensive fetches.

## 5. AGENTS.md content

A cloud-ready root `AGENTS.md` should contain:

1. Mission and decision priority.
2. Required reading order and excluded/non-normative folders.
3. Non-negotiable domain invariants.
4. Repository/component authority boundaries.
5. Task claim and path ownership protocol.
6. Engineering and contract-generation workflow.
7. Test expectations and network-test policy.
8. Secrets, privacy, data, and logging constraints.
9. Pull-request evidence/handoff requirements.

Avoid generic exhortations such as “write clean code.” Include rules that change a capable agent's decisions in this repository.

## 6. Parallel task graph

Each task entry includes:

- stable ID and short title;
- status (`ready`, `active`, `blocked`, `done`);
- merged dependencies;
- exclusive owned paths;
- deliverables;
- observable acceptance checks.

Recommended sequencing:

1. Monorepo/toolchain bootstrap.
2. Contract source and generation.
3. CI/policy and synthetic fixtures.
4. Independent domain, adapter, API, and UI foundations.
5. Integration, end-to-end, performance, security, and release evidence.

Task size should fit one coherent review. Split oversized tasks into child IDs with non-overlapping paths and an explicit merge order. Cross-cutting contract changes land first, then consumers update in parallel.

Create an issue template that asks for task ID, dependency evidence, claimed paths, and acceptance plan. Seed the first ready issue only when external issue creation is authorized or is an ordinary part of the requested public-repository setup.

## 7. Repository hygiene

- Add ignores for secrets, local databases, build products, provider caches, and large generated data appropriate to the stack.
- Choose a license deliberately and disclose the choice; code licensing does not grant rights to third-party data/assets.
- Preserve the user's exact request only if asked, and explicitly mark whether agents should treat it as normative.
- Use relative links inside repository Markdown and verify them.
- Prefer current official sources linked near provider/technical claims, including an “as checked” date when capabilities can change.
- Keep the initial commit coherent: specification, task graph, agent guide, contracts/fixtures only when useful, and no accidental product implementation.
