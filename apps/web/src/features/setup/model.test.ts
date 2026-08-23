import assert from "node:assert/strict";
import test from "node:test";

import {
  beginPreflight,
  buildCommitPayload,
  canCommit,
  canonicalI64,
  commitBlockers,
  completePreflight,
  createSetupState,
  markCommitted,
  requiredCapabilities,
  setupRequestKey,
  updateSetupDraft,
  validateSetup,
} from "./model.ts";
import type { SetupDraft, SetupPreflight, SetupState } from "./model.ts";

function draft(overrides: Partial<SetupDraft> = {}): SetupDraft {
  return {
    mode: "HISTORICAL",
    universe: "synthetic-crypto",
    instrumentId: "SYNTH-BTC-USD",
    manifestHash: "a".repeat(64),
    playStartNs: "1700000000000000000",
    warmupNs: "60000000000",
    durationNs: "3600000000000",
    equityMinor: "1000000",
    leverage: 5,
    executionTier: "F1",
    chartInterval: "5m",
    rulesetId: "default",
    rulesetVersion: "1",
    rulesetHash: "b".repeat(64),
    rulesetAllowedTiers: ["F1", "F2"],
    visibilityMode: "RELATIVE",
    extraRequiredCapabilities: [],
    allowedRedistribution: ["REDISTRIBUTABLE", "USER_LICENSED"],
    allowDegraded: false,
    ...overrides,
  };
}

function eligibleReport(
  state: SetupState,
  overrides: Partial<SetupPreflight> = {},
): SetupPreflight {
  return {
    requestKey: setupRequestKey(state.draft),
    eligible: true,
    supportedExecutionTiers: ["F1", "F2"],
    availableCapabilities: ["BBO", "L2_DELTAS", "L2_SNAPSHOTS", "TRADES"],
    authenticLeverageCap: 20,
    estimatedCostMinor: "0",
    costCurrency: "USD",
    gaps: [],
    warnings: [],
    rejectionReasons: [],
    ...overrides,
  };
}

function readyState(overrides: Partial<SetupDraft> = {}): SetupState {
  const initial = createSetupState(draft(overrides));
  const checking = beginPreflight(initial);
  return completePreflight(checking, eligibleReport(checking));
}

test("canonical signed integers reject floats, plus signs, negative zero, and unsafe range", () => {
  assert.equal(canonicalI64("0"), "0");
  assert.equal(canonicalI64("9223372036854775807"), "9223372036854775807");
  assert.equal(canonicalI64("-9223372036854775808"), "-9223372036854775808");
  for (const invalid of ["1.5", "+1", "01", "-0", "9223372036854775808"]) {
    assert.throws(() => canonicalI64(invalid), RangeError);
  }
});

test("tier capabilities match the API setup contract", () => {
  assert.deepEqual(requiredCapabilities(draft({ executionTier: "F0" })), ["BARS"]);
  assert.deepEqual(requiredCapabilities(draft({ executionTier: "F0T" })), ["TRADES"]);
  assert.deepEqual(requiredCapabilities(draft({ executionTier: "F1" })), ["BBO", "TRADES"]);
  assert.deepEqual(requiredCapabilities(draft({ executionTier: "F2" })), [
    "L2_DELTAS",
    "L2_SNAPSHOTS",
    "TRADES",
  ]);
  assert.deepEqual(requiredCapabilities(draft({ executionTier: "F3" })), ["L3"]);
});

test("setup validation never coerces exact financial or time inputs through Number", () => {
  assert.deepEqual(validateSetup(draft()), {});
  const invalid = validateSetup(draft({ equityMinor: "100.00", durationNs: "01", leverage: 5.5 }));
  assert.ok(invalid.equityMinor);
  assert.ok(invalid.durationNs);
  assert.ok(invalid.leverage);
});

test("changing any preflight-sensitive field invalidates a previously eligible response", () => {
  const ready = readyState();
  assert.equal(canCommit(ready), true);
  const changed = updateSetupDraft(ready, { durationNs: "7200000000000" });
  assert.equal(changed.preflight.status, "IDLE");
  assert.equal(canCommit(changed), false);
});

test("stale preflight response is rejected instead of enabling commit", () => {
  const initial = createSetupState(draft());
  const checking = beginPreflight(initial);
  const stale = eligibleReport(checking);
  const changed = updateSetupDraft(checking, { leverage: 6 });
  assert.throws(
    () => completePreflight(changed, stale),
    /Stale preflight result does not match the current setup/,
  );
});

test("authentic leverage cap can be stricter than the synthetic 50x ceiling", () => {
  const initial = createSetupState(draft({ leverage: 20 }));
  const checking = beginPreflight(initial);
  const capped = completePreflight(
    checking,
    eligibleReport(checking, { authenticLeverageCap: 10 }),
  );
  assert.equal(canCommit(capped), false);
  assert.ok(commitBlockers(capped).some((message) => message.includes("10×")));
});

test("missing fidelity capabilities block a nominally eligible preflight", () => {
  const initial = createSetupState(draft({ executionTier: "F2" }));
  const checking = beginPreflight(initial);
  const missingDepth = completePreflight(
    checking,
    eligibleReport(checking, {
      supportedExecutionTiers: ["F2"],
      availableCapabilities: ["TRADES"],
    }),
  );
  assert.equal(canCommit(missingDepth), false);
  assert.ok(commitBlockers(missingDepth).some((message) => message.includes("L2_DELTAS")));
});

test("commit payload preserves canonical wire integers and server setup vocabulary", () => {
  const state = readyState();
  const payload = buildCommitPayload(state);
  assert.deepEqual(payload.session_setup.required_capabilities, ["BBO", "TRADES"]);
  assert.equal(payload.session_setup.play_start_ns, "1700000000000000000");
  assert.equal(payload.session_setup.duration_ns, "3600000000000");
  assert.equal(payload.trading_setup.equity_minor, "1000000");
  assert.equal(payload.trading_setup.leverage, 5);
  assert.equal(typeof payload.session_setup.play_start_ns, "string");
});

test("successful commit locks every setup mutation", () => {
  const locked = markCommitted(readyState());
  assert.equal(locked.locked, true);
  assert.throws(() => updateSetupDraft(locked, { leverage: 6 }), /Committed setup is immutable/);
});

test("ineligible catalog report surfaces stable rejection reasons", () => {
  const initial = createSetupState(draft());
  const checking = beginPreflight(initial);
  const state = completePreflight(
    checking,
    eligibleReport(checking, {
      eligible: false,
      rejectionReasons: ["KNOWN_GAP_INTERSECTS_PLAY"],
      gaps: [{ startNs: "1700000000000000001", endNs: "1700000000000000100", reason: "gap" }],
    }),
  );
  assert.equal(canCommit(state), false);
  assert.ok(commitBlockers(state).includes("KNOWN_GAP_INTERSECTS_PLAY"));
});
