export type SessionMode = "HISTORICAL" | "RANDOM";
export type ExecutionTier = "F0" | "F0T" | "F1" | "F2" | "F3";
export type VisibilityMode = "ABSOLUTE" | "RELATIVE" | "HIDDEN_CALENDAR";
export type RedistributionClass =
  | "REDISTRIBUTABLE"
  | "USER_LICENSED"
  | "RESTRICTED"
  | "UNKNOWN";
export type DataCapability =
  | "BARS"
  | "TRADES"
  | "BBO"
  | "L2_SNAPSHOTS"
  | "L2_DELTAS"
  | "L3"
  | "MARK_PRICE"
  | "INDEX_PRICE"
  | "FUNDING"
  | "OPEN_INTEREST"
  | "LIQUIDATIONS";
export type ChartInterval = "1m" | "5m" | "15m" | "1h" | "1d";

const I64_MIN = -(1n << 63n);
const I64_MAX = (1n << 63n) - 1n;
const SHA256 = /^[0-9a-f]{64}$/;
const CANONICAL_INTEGER = /^(?:0|-?[1-9][0-9]*)$/;

export const TIER_CAPABILITIES: Readonly<Record<ExecutionTier, readonly DataCapability[]>> = {
  F0: ["BARS"],
  F0T: ["TRADES"],
  F1: ["BBO", "TRADES"],
  F2: ["L2_DELTAS", "L2_SNAPSHOTS", "TRADES"],
  F3: ["L3"],
};

export type SetupDraft = {
  mode: SessionMode;
  universe: string;
  instrumentId: string;
  manifestHash: string;
  playStartNs: string;
  warmupNs: string;
  durationNs: string;
  equityMinor: string;
  leverage: number;
  executionTier: ExecutionTier;
  chartInterval: ChartInterval;
  rulesetId: string;
  rulesetVersion: string;
  rulesetHash: string;
  rulesetAllowedTiers: readonly ExecutionTier[];
  visibilityMode: VisibilityMode;
  extraRequiredCapabilities: readonly DataCapability[];
  allowedRedistribution: readonly RedistributionClass[];
  allowDegraded: boolean;
};

export type CoverageGap = {
  startNs: string;
  endNs: string;
  reason: string;
};

export type SetupPreflight = {
  requestKey: string;
  eligible: boolean;
  supportedExecutionTiers: readonly ExecutionTier[];
  availableCapabilities: readonly DataCapability[];
  authenticLeverageCap: number | null;
  estimatedCostMinor: string | null;
  costCurrency: string | null;
  gaps: readonly CoverageGap[];
  warnings: readonly string[];
  rejectionReasons: readonly string[];
};

export type PreflightState =
  | { status: "IDLE" }
  | { status: "CHECKING"; requestKey: string }
  | { status: "READY"; report: SetupPreflight }
  | { status: "ERROR"; requestKey: string; message: string };

export type SetupState = {
  draft: SetupDraft;
  preflight: PreflightState;
  locked: boolean;
};

export type SetupCommitPayload = {
  mode: SessionMode;
  universe: string;
  session_setup: {
    instrument_id: string;
    manifest_hash: string;
    play_start_ns: string;
    warmup_ns: string;
    duration_ns: string;
    execution_tier: ExecutionTier;
    required_capabilities: readonly DataCapability[];
    allowed_redistribution: readonly RedistributionClass[];
    allow_degraded: boolean;
    visibility_mode: VisibilityMode;
    ruleset_id: string;
    ruleset_version: string;
    ruleset_hash: string;
  };
  trading_setup: {
    equity_minor: string;
    leverage: number;
    chart_interval: ChartInterval;
  };
};

export type SetupValidation = Readonly<Record<string, string>>;

export function createSetupState(draft: SetupDraft): SetupState {
  return { draft: normalizeDraft(draft), preflight: { status: "IDLE" }, locked: false };
}

export function normalizeDraft(draft: SetupDraft): SetupDraft {
  return {
    ...draft,
    rulesetAllowedTiers: sortedUnique(draft.rulesetAllowedTiers),
    extraRequiredCapabilities: sortedUnique(draft.extraRequiredCapabilities),
    allowedRedistribution: sortedUnique(draft.allowedRedistribution),
  };
}

export function requiredCapabilities(draft: SetupDraft): readonly DataCapability[] {
  return sortedUnique([
    ...TIER_CAPABILITIES[draft.executionTier],
    ...draft.extraRequiredCapabilities,
  ]);
}

export function validateSetup(draft: SetupDraft): SetupValidation {
  const errors: Record<string, string> = {};
  requiredText(errors, "universe", draft.universe);
  requiredText(errors, "instrumentId", draft.instrumentId);
  requiredText(errors, "rulesetId", draft.rulesetId);
  requiredText(errors, "rulesetVersion", draft.rulesetVersion);
  validateHash(errors, "manifestHash", draft.manifestHash);
  validateHash(errors, "rulesetHash", draft.rulesetHash);

  const playStart = validateI64Field(errors, "playStartNs", draft.playStartNs);
  const warmup = validateI64Field(errors, "warmupNs", draft.warmupNs);
  const duration = validateI64Field(errors, "durationNs", draft.durationNs);
  const equity = validateI64Field(errors, "equityMinor", draft.equityMinor);

  if (warmup !== null && warmup < 0n) {
    errors.warmupNs = "Warm-up cannot be negative.";
  }
  if (duration !== null && duration <= 0n) {
    errors.durationNs = "Duration must be positive.";
  }
  if (equity !== null && equity <= 0n) {
    errors.equityMinor = "Starting equity must be positive.";
  }
  if (playStart !== null && warmup !== null && !fitsI64(playStart - warmup)) {
    errors.playStartNs = "Warm-up start exceeds the signed 64-bit time range.";
  }
  if (playStart !== null && duration !== null && !fitsI64(playStart + duration)) {
    errors.durationNs = "Replay end exceeds the signed 64-bit time range.";
  }
  if (!Number.isInteger(draft.leverage) || draft.leverage < 1 || draft.leverage > 50) {
    errors.leverage = "Leverage must be an integer from 1× through 50×.";
  }
  if (!draft.rulesetAllowedTiers.includes(draft.executionTier)) {
    errors.executionTier = "The selected ruleset does not permit this execution fidelity.";
  }
  if (draft.allowedRedistribution.length === 0) {
    errors.allowedRedistribution = "At least one data-rights class must be allowed.";
  }
  if (!isSortedUnique(draft.rulesetAllowedTiers)) {
    errors.rulesetAllowedTiers = "Ruleset tiers must be unique and canonically ordered.";
  }
  if (!isSortedUnique(draft.extraRequiredCapabilities)) {
    errors.extraRequiredCapabilities = "Capabilities must be unique and canonically ordered.";
  }
  if (!isSortedUnique(draft.allowedRedistribution)) {
    errors.allowedRedistribution = "Data-rights classes must be unique and canonically ordered.";
  }
  return errors;
}

export function setupRequestKey(draft: SetupDraft): string {
  const normalized = normalizeDraft(draft);
  return [
    "trl-setup-preflight-v1",
    normalized.mode,
    normalized.universe,
    normalized.instrumentId,
    normalized.manifestHash,
    normalized.playStartNs,
    normalized.warmupNs,
    normalized.durationNs,
    normalized.equityMinor,
    String(normalized.leverage),
    normalized.executionTier,
    normalized.chartInterval,
    normalized.rulesetId,
    normalized.rulesetVersion,
    normalized.rulesetHash,
    normalized.rulesetAllowedTiers.join(","),
    normalized.visibilityMode,
    normalized.extraRequiredCapabilities.join(","),
    normalized.allowedRedistribution.join(","),
    normalized.allowDegraded ? "1" : "0",
  ].join("\u001f");
}

export function updateSetupDraft(state: SetupState, patch: Partial<SetupDraft>): SetupState {
  if (state.locked) {
    throw new Error("Committed setup is immutable.");
  }
  const nextDraft = normalizeDraft({ ...state.draft, ...patch });
  const changed = setupRequestKey(nextDraft) !== setupRequestKey(state.draft);
  return {
    ...state,
    draft: nextDraft,
    preflight: changed ? { status: "IDLE" } : state.preflight,
  };
}

export function beginPreflight(state: SetupState): SetupState {
  ensureUnlocked(state);
  const errors = validateSetup(state.draft);
  if (Object.keys(errors).length > 0) {
    throw new Error("Setup must be valid before preflight.");
  }
  return {
    ...state,
    preflight: { status: "CHECKING", requestKey: setupRequestKey(state.draft) },
  };
}

export function completePreflight(state: SetupState, report: SetupPreflight): SetupState {
  ensureUnlocked(state);
  if (report.requestKey !== setupRequestKey(state.draft)) {
    throw new Error("Stale preflight result does not match the current setup.");
  }
  validatePreflight(report);
  return { ...state, preflight: { status: "READY", report } };
}

export function failPreflight(state: SetupState, message: string): SetupState {
  ensureUnlocked(state);
  if (!message) {
    throw new Error("Preflight error message is required.");
  }
  return {
    ...state,
    preflight: { status: "ERROR", requestKey: setupRequestKey(state.draft), message },
  };
}

export function effectiveLeverageCap(state: SetupState): number {
  if (state.preflight.status !== "READY" || state.preflight.report.authenticLeverageCap === null) {
    return 50;
  }
  return Math.min(50, state.preflight.report.authenticLeverageCap);
}

export function commitBlockers(state: SetupState): readonly string[] {
  const blockers = Object.values(validateSetup(state.draft));
  if (state.locked) {
    blockers.push("Setup is already committed and locked.");
    return blockers;
  }
  if (state.preflight.status !== "READY") {
    blockers.push("Run preflight against current catalog coverage before committing.");
    return blockers;
  }
  const report = state.preflight.report;
  if (report.requestKey !== setupRequestKey(state.draft)) {
    blockers.push("Preflight is stale for the current setup.");
  }
  if (!report.eligible) {
    blockers.push(...report.rejectionReasons);
    if (report.rejectionReasons.length === 0) {
      blockers.push("Catalog preflight marked this setup ineligible.");
    }
  }
  if (!report.supportedExecutionTiers.includes(state.draft.executionTier)) {
    blockers.push("Selected dataset does not support the requested execution fidelity.");
  }
  const available = new Set(report.availableCapabilities);
  const missing = requiredCapabilities(state.draft).filter((item) => !available.has(item));
  if (missing.length > 0) {
    blockers.push(`Missing required data capabilities: ${missing.join(", ")}.`);
  }
  if (state.draft.leverage > effectiveLeverageCap(state)) {
    blockers.push(`Authentic leverage cap is ${effectiveLeverageCap(state)}× for this setup.`);
  }
  return sortedUnique(blockers);
}

export function canCommit(state: SetupState): boolean {
  return commitBlockers(state).length === 0;
}

export function buildCommitPayload(state: SetupState): SetupCommitPayload {
  if (!canCommit(state)) {
    throw new Error("Setup is not eligible to commit.");
  }
  const draft = state.draft;
  return {
    mode: draft.mode,
    universe: draft.universe,
    session_setup: {
      instrument_id: draft.instrumentId,
      manifest_hash: draft.manifestHash,
      play_start_ns: canonicalI64(draft.playStartNs),
      warmup_ns: canonicalI64(draft.warmupNs),
      duration_ns: canonicalI64(draft.durationNs),
      execution_tier: draft.executionTier,
      required_capabilities: requiredCapabilities(draft),
      allowed_redistribution: draft.allowedRedistribution,
      allow_degraded: draft.allowDegraded,
      visibility_mode: draft.visibilityMode,
      ruleset_id: draft.rulesetId,
      ruleset_version: draft.rulesetVersion,
      ruleset_hash: draft.rulesetHash,
    },
    trading_setup: {
      equity_minor: canonicalI64(draft.equityMinor),
      leverage: draft.leverage,
      chart_interval: draft.chartInterval,
    },
  };
}

export function markCommitted(state: SetupState): SetupState {
  if (!canCommit(state)) {
    throw new Error("Setup cannot be locked before a successful eligible preflight.");
  }
  return { ...state, locked: true };
}

export function canonicalI64(value: string): string {
  if (!CANONICAL_INTEGER.test(value)) {
    throw new RangeError("value must use canonical decimal integer encoding");
  }
  const parsed = BigInt(value);
  if (!fitsI64(parsed)) {
    throw new RangeError("value must fit signed 64-bit integer");
  }
  return parsed.toString(10);
}

function validatePreflight(report: SetupPreflight): void {
  if (!report.requestKey) {
    throw new Error("Preflight request key is required.");
  }
  if (report.authenticLeverageCap !== null) {
    if (
      !Number.isInteger(report.authenticLeverageCap) ||
      report.authenticLeverageCap < 1 ||
      report.authenticLeverageCap > 50
    ) {
      throw new Error("Authentic leverage cap must be an integer from 1× through 50×.");
    }
  }
  if (report.estimatedCostMinor !== null) {
    canonicalI64(report.estimatedCostMinor);
    if (BigInt(report.estimatedCostMinor) < 0n) {
      throw new Error("Estimated cost cannot be negative.");
    }
    if (!report.costCurrency) {
      throw new Error("Cost currency is required when an estimate is present.");
    }
  }
  if (!isSortedUnique(report.supportedExecutionTiers)) {
    throw new Error("Supported execution tiers must be unique and canonically ordered.");
  }
  if (!isSortedUnique(report.availableCapabilities)) {
    throw new Error("Available capabilities must be unique and canonically ordered.");
  }
  for (const gap of report.gaps) {
    const start = BigInt(canonicalI64(gap.startNs));
    const end = BigInt(canonicalI64(gap.endNs));
    if (end <= start || !gap.reason) {
      throw new Error("Preflight gaps must have increasing bounds and a reason.");
    }
  }
}

function validateI64Field(
  errors: Record<string, string>,
  key: string,
  value: string,
): bigint | null {
  try {
    return BigInt(canonicalI64(value));
  } catch {
    errors[key] = "Use a canonical signed 64-bit decimal integer.";
    return null;
  }
}

function validateHash(errors: Record<string, string>, key: string, value: string): void {
  if (!SHA256.test(value)) {
    errors[key] = "Use a lowercase SHA-256 hex digest.";
  }
}

function requiredText(errors: Record<string, string>, key: string, value: string): void {
  if (!value.trim()) {
    errors[key] = "This field is required.";
  }
}

function fitsI64(value: bigint): boolean {
  return value >= I64_MIN && value <= I64_MAX;
}

function ensureUnlocked(state: SetupState): void {
  if (state.locked) {
    throw new Error("Committed setup is immutable.");
  }
}

function sortedUnique<T extends string>(values: readonly T[]): readonly T[] {
  return [...new Set(values)].sort();
}

function isSortedUnique<T extends string>(values: readonly T[]): boolean {
  const canonical = sortedUnique(values);
  return canonical.length === values.length && canonical.every((value, index) => value === values[index]);
}
