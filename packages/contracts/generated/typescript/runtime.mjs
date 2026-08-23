const JS_SAFE_MAX = BigInt(Number.MAX_SAFE_INTEGER);
const INT64_MIN = -(1n << 63n);
const INT64_MAX = (1n << 63n) - 1n;
const UINT64_MAX = (1n << 64n) - 1n;
const unsignedNames = new Set([
  "arrival_seq", "event_seq", "source_sequence", "canonical_tie_breaker",
  "submitted_at_event_seq", "expected_session_version", "generation", "trade_count",
  "duplicates_removed", "timestamp_resolution_ns",
]);

function wireInteger(value, signed, path) {
  if (typeof value !== "string" || !/^-?(0|[1-9][0-9]*)$/.test(value) || value === "-0") {
    throw new Error(`${path}: non-canonical wire integer`);
  }
  const parsed = BigInt(value);
  const min = signed ? INT64_MIN : 0n;
  const max = signed ? INT64_MAX : UINT64_MAX;
  if (parsed < min || parsed > max) throw new Error(`${path}: integer out of range`);
}

export function validateDocument(value, path = "$") {
  if (Array.isArray(value)) {
    value.forEach((item, index) => validateDocument(item, `${path}[${index}]`));
    return;
  }
  if (value === null || typeof value !== "object") {
    if (typeof value === "number" && (!Number.isInteger(value) || BigInt(Math.abs(value)) > JS_SAFE_MAX)) {
      throw new Error(`${path}: float or unsafe JSON integer`);
    }
    return;
  }
  if ("schema_version" in value && value.schema_version !== "1.0.0") {
    throw new Error(`${path}.schema_version: unsupported`);
  }
  for (const [key, item] of Object.entries(value)) {
    const child = `${path}.${key}`;
    if (unsignedNames.has(key)) wireInteger(item, false, child);
    else if (key.endsWith("_atoms") || key.endsWith("_minor") || key.endsWith("_ppb") || key.endsWith("_ns")) {
      const signed = !key.startsWith("qty_") && !key.endsWith("volume_atoms");
      wireInteger(item, signed, child);
    }
    validateDocument(item, child);
  }
  if (value.command_type === "ORDER" && value.post_only && value.marketable_only) {
    throw new Error(`${path}: mutually exclusive order flags`);
  }
}

function sorted(value) {
  if (Array.isArray(value)) return value.map(sorted);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sorted(value[key])]));
  }
  return value;
}

export function canonicalJson(value) {
  validateDocument(value);
  return JSON.stringify(sorted(value));
}

if (process.argv[1] && import.meta.url.endsWith(process.argv[1].replaceAll("\\", "/"))) {
  const { readFile } = await import("node:fs/promises");
  const value = JSON.parse(await readFile(process.argv[2], "utf8"));
  process.stdout.write(`${canonicalJson(value)}\n`);
}
