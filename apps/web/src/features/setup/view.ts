"use client";

import { createElement, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";

import { Button, Dialog, StatusBadge } from "../../components/foundation/index";
import {
  canCommit,
  commitBlockers,
  effectiveLeverageCap,
  requiredCapabilities,
  validateSetup,
} from "./model";
import type {
  ChartInterval,
  ExecutionTier,
  SessionMode,
  SetupDraft,
  SetupState,
  VisibilityMode,
} from "./model";

export type SetupFormProps = {
  state: SetupState;
  onPatch: (patch: Partial<SetupDraft>) => void;
  onPreflight: () => void;
  onCommit: () => void;
};

const EXECUTION_TIERS: readonly ExecutionTier[] = ["F0", "F0T", "F1", "F2", "F3"];
const CHART_INTERVALS: readonly ChartInterval[] = ["1m", "5m", "15m", "1h", "1d"];
const VISIBILITY_MODES: readonly VisibilityMode[] = ["ABSOLUTE", "RELATIVE", "HIDDEN_CALENDAR"];
const SESSION_MODES: readonly SessionMode[] = ["HISTORICAL", "RANDOM"];

export function SetupForm({ state, onPatch, onPreflight, onCommit }: SetupFormProps) {
  const [confirming, setConfirming] = useState(false);
  const errors = validateSetup(state.draft);
  const blockers = commitBlockers(state);
  const disabled = state.locked;
  const preflightDisabled = disabled || Object.keys(errors).length > 0;
  const commitDisabled = !canCommit(state);

  return createElement(
    "section",
    { className: "trl-setup", "aria-labelledby": "setup-title" },
    createElement("h1", { id: "setup-title" }, "Replay setup"),
    createElement(
      "p",
      { className: "trl-setup__intro" },
      "Preflight validates coverage, fidelity, gaps, data rights, and authentic leverage before setup can be committed.",
    ),
    state.locked
      ? createElement(
          "p",
          { role: "status", "aria-live": "polite" },
          createElement(StatusBadge, null, "Committed · setup locked"),
        )
      : null,
    createElement(
      "form",
      {
        onSubmit: (event) => event.preventDefault(),
        "aria-describedby": blockers.length > 0 ? "setup-blockers" : undefined,
      },
      createElement(
        "fieldset",
        { disabled, className: "trl-setup__group" },
        createElement("legend", null, "Episode"),
        selectField("setup-mode", "Mode", state.draft.mode, SESSION_MODES, (value) =>
          onPatch({ mode: value as SessionMode }),
        ),
        textField("setup-universe", "Universe", state.draft.universe, errors.universe, (value) =>
          onPatch({ universe: value }),
        ),
        textField(
          "setup-instrument",
          "Instrument",
          state.draft.instrumentId,
          errors.instrumentId,
          (value) => onPatch({ instrumentId: value }),
        ),
        textField(
          "setup-manifest",
          "Dataset manifest SHA-256",
          state.draft.manifestHash,
          errors.manifestHash,
          (value) => onPatch({ manifestHash: value }),
        ),
        textField(
          "setup-start",
          "Play start, nanoseconds",
          state.draft.playStartNs,
          errors.playStartNs,
          (value) => onPatch({ playStartNs: value }),
          "numeric",
        ),
        textField(
          "setup-warmup",
          "Warm-up, nanoseconds",
          state.draft.warmupNs,
          errors.warmupNs,
          (value) => onPatch({ warmupNs: value }),
          "numeric",
        ),
        textField(
          "setup-duration",
          "Duration, nanoseconds",
          state.draft.durationNs,
          errors.durationNs,
          (value) => onPatch({ durationNs: value }),
          "numeric",
        ),
      ),
      createElement(
        "fieldset",
        { disabled, className: "trl-setup__group" },
        createElement("legend", null, "Trading and fidelity"),
        textField(
          "setup-equity",
          "Starting equity, minor units",
          state.draft.equityMinor,
          errors.equityMinor,
          (value) => onPatch({ equityMinor: value }),
          "numeric",
        ),
        createElement(
          "div",
          { className: "trl-field" },
          createElement("label", { htmlFor: "setup-leverage" }, "Leverage"),
          createElement("input", {
            id: "setup-leverage",
            type: "number",
            min: 1,
            max: 50,
            step: 1,
            value: state.draft.leverage,
            "aria-invalid": errors.leverage ? true : undefined,
            "aria-describedby": errors.leverage ? "setup-leverage-error" : "setup-leverage-help",
            onChange: (event: ChangeEvent<HTMLInputElement>) =>
              onPatch({ leverage: Number(event.currentTarget.value) }),
          }),
          createElement(
            "small",
            { id: "setup-leverage-help" },
            `Synthetic range: 1×–50×. Current authentic cap after preflight: ${effectiveLeverageCap(state)}×.`,
          ),
          errorNode("setup-leverage-error", errors.leverage),
        ),
        selectField(
          "setup-tier",
          "Execution fidelity",
          state.draft.executionTier,
          EXECUTION_TIERS,
          (value) => onPatch({ executionTier: value as ExecutionTier }),
          errors.executionTier,
        ),
        createElement(
          "p",
          { className: "trl-field__help" },
          `Minimum capabilities: ${requiredCapabilities(state.draft).join(", ")}.`,
        ),
        selectField(
          "setup-chart",
          "Coarse chart interval",
          state.draft.chartInterval,
          CHART_INTERVALS,
          (value) => onPatch({ chartInterval: value as ChartInterval }),
        ),
      ),
      createElement(
        "fieldset",
        { disabled, className: "trl-setup__group" },
        createElement("legend", null, "Rules and information"),
        textField("setup-ruleset-id", "Ruleset", state.draft.rulesetId, errors.rulesetId, (value) =>
          onPatch({ rulesetId: value }),
        ),
        textField(
          "setup-ruleset-version",
          "Ruleset version",
          state.draft.rulesetVersion,
          errors.rulesetVersion,
          (value) => onPatch({ rulesetVersion: value }),
        ),
        textField(
          "setup-ruleset-hash",
          "Ruleset SHA-256",
          state.draft.rulesetHash,
          errors.rulesetHash,
          (value) => onPatch({ rulesetHash: value }),
        ),
        selectField(
          "setup-visibility",
          "Calendar visibility",
          state.draft.visibilityMode,
          VISIBILITY_MODES,
          (value) => onPatch({ visibilityMode: value as VisibilityMode }),
        ),
        createElement(
          "label",
          { className: "trl-checkbox" },
          createElement("input", {
            type: "checkbox",
            checked: state.draft.allowDegraded,
            onChange: (event: ChangeEvent<HTMLInputElement>) =>
              onPatch({ allowDegraded: event.currentTarget.checked }),
          }),
          "Allow explicitly degraded catalog coverage",
        ),
      ),
      createElement(
        "div",
        { className: "trl-setup__actions" },
        createElement(
          Button,
          { variant: "secondary", disabled: preflightDisabled, onClick: onPreflight },
          state.preflight.status === "CHECKING" ? "Checking…" : "Run preflight",
        ),
        createElement(
          Button,
          {
            disabled: commitDisabled,
            onClick: () => setConfirming(true),
            "aria-describedby": blockers.length > 0 ? "setup-blockers" : undefined,
          },
          "Commit setup",
        ),
      ),
    ),
    preflightNode(state),
    blockers.length > 0
      ? createElement(
          "div",
          { id: "setup-blockers", role: "status", "aria-live": "polite" },
          createElement("h2", null, "Before you can commit"),
          createElement(
            "ul",
            null,
            ...blockers.map((blocker) => createElement("li", { key: blocker }, blocker)),
          ),
        )
      : null,
    confirming
      ? createElement(
          Dialog,
          {
            open: true,
            title: "Commit replay setup?",
            titleId: "commit-setup-title",
            onClose: () => setConfirming(false),
          },
          createElement(
            "p",
            null,
            "Committing pins the dataset, ruleset, fidelity, information policy, timing, equity, and leverage. These setup choices cannot be edited afterward.",
          ),
          createElement(
            "div",
            { className: "trl-setup__actions" },
            createElement(
              Button,
              { variant: "secondary", onClick: () => setConfirming(false) },
              "Go back",
            ),
            createElement(
              Button,
              {
                onClick: () => {
                  setConfirming(false);
                  onCommit();
                },
              },
              "Confirm and lock setup",
            ),
          ),
        )
      : null,
  );
}

function preflightNode(state: SetupState): ReactNode {
  if (state.preflight.status === "IDLE") {
    return null;
  }
  if (state.preflight.status === "CHECKING") {
    return createElement("p", { role: "status", "aria-live": "polite" }, "Checking eligibility…");
  }
  if (state.preflight.status === "ERROR") {
    return createElement("p", { role: "alert" }, `Preflight failed: ${state.preflight.message}`);
  }

  const report = state.preflight.report;
  return createElement(
    "section",
    { className: "trl-setup__preflight", "aria-labelledby": "preflight-title" },
    createElement("h2", { id: "preflight-title" }, "Preflight"),
    createElement(
      "p",
      { role: "status", "aria-live": "polite" },
      createElement(StatusBadge, null, report.eligible ? "Eligible" : "Ineligible"),
    ),
    report.estimatedCostMinor === null
      ? createElement("p", null, "Provider cost: no metered estimate declared.")
      : createElement(
          "p",
          null,
          `Estimated provider cost: ${report.estimatedCostMinor} ${report.costCurrency ?? ""} minor units.`,
        ),
    createElement(
      "p",
      null,
      `Supported fidelity: ${report.supportedExecutionTiers.join(", ") || "none"}.`,
    ),
    createElement(
      "p",
      null,
      `Available capabilities: ${report.availableCapabilities.join(", ") || "none"}.`,
    ),
    report.gaps.length > 0
      ? createElement(
          "div",
          null,
          createElement("h3", null, "Known coverage gaps"),
          createElement(
            "ul",
            null,
            ...report.gaps.map((gap) =>
              createElement(
                "li",
                { key: `${gap.startNs}:${gap.endNs}:${gap.reason}` },
                `${gap.startNs}–${gap.endNs} ns: ${gap.reason}`,
              ),
            ),
          ),
        )
      : createElement("p", null, "No known gaps intersect the requested coverage."),
    report.warnings.length > 0
      ? createElement(
          "div",
          null,
          createElement("h3", null, "Fidelity warnings"),
          createElement(
            "ul",
            null,
            ...report.warnings.map((warning) => createElement("li", { key: warning }, warning)),
          ),
        )
      : null,
  );
}

function textField(
  id: string,
  label: string,
  value: string,
  error: string | undefined,
  onChange: (value: string) => void,
  inputMode?: "numeric",
): ReactNode {
  const errorId = `${id}-error`;
  return createElement(
    "div",
    { className: "trl-field" },
    createElement("label", { htmlFor: id }, label),
    createElement("input", {
      id,
      value,
      inputMode,
      spellCheck: false,
      "aria-invalid": error ? true : undefined,
      "aria-describedby": error ? errorId : undefined,
      onChange: (event: ChangeEvent<HTMLInputElement>) => onChange(event.currentTarget.value),
    }),
    errorNode(errorId, error),
  );
}

function selectField(
  id: string,
  label: string,
  value: string,
  options: readonly string[],
  onChange: (value: string) => void,
  error?: string,
): ReactNode {
  const errorId = `${id}-error`;
  return createElement(
    "div",
    { className: "trl-field" },
    createElement("label", { htmlFor: id }, label),
    createElement(
      "select",
      {
        id,
        value,
        "aria-invalid": error ? true : undefined,
        "aria-describedby": error ? errorId : undefined,
        onChange: (event: ChangeEvent<HTMLSelectElement>) => onChange(event.currentTarget.value),
      },
      ...options.map((option) => createElement("option", { key: option, value: option }, option)),
    ),
    errorNode(errorId, error),
  );
}

function errorNode(id: string, error: string | undefined): ReactNode {
  return error ? createElement("small", { id, role: "alert" }, error) : null;
}
