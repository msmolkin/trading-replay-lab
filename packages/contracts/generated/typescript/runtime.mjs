const JS_SAFE_MAX = BigInt(Number.MAX_SAFE_INTEGER);
const INT64_MIN = -(1n << 63n);
const INT64_MAX = (1n << 63n) - 1n;
const UINT64_MAX = (1n << 64n) - 1n;
const unsignedNames = new Set([
  "arrival_seq", "event_seq", "source_sequence", "canonical_tie_breaker",
  "submitted_at_event_seq", "expected_session_version", "generation", "trade_count",
  "duplicates_removed", "timestamp_resolution_ns",
]);
const canonicalCommandTypes = new Set([
  "SUBMIT_ORDER", "CANCEL_ORDER", "REPLACE_ORDER", "SET_LEVERAGE",
]);
const orderTypes = new Set(["MARKET", "LIMIT", "STOP_MARKET", "STOP_LIMIT"]);
const timeInForce = new Set(["GTC", "IOC", "FOK"]);
const priceReferences = new Set(["BID", "ASK", "MIDPOINT"]);

function wireInteger(value, signed, path) {
  if (typeof value !== "string" || !/^-?(0|[1-9][0-9]*)$/.test(value) || value === "-0") {
    throw new Error(`${path}: non-canonical wire integer`);
  }
  const parsed = BigInt(value);
  const min = signed ? INT64_MIN : 0n;
  const max = signed ? INT64_MAX : UINT64_MAX;
  if (parsed < min || parsed > max) throw new Error(`${path}: integer out of range`);
  return parsed;
}

function positiveWire(value, signed, path) {
  const parsed = wireInteger(value, signed, path);
  if (parsed <= 0n) throw new Error(`${path}: must be positive`);
}

function nonemptyText(value, path) {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${path}: must be non-empty`);
}

function booleanValue(value, path) {
  if (typeof value !== "boolean") throw new Error(`${path}: must be boolean`);
}

function exactKeys(value, required, allowed, path) {
  for (const key of required) {
    if (!(key in value)) throw new Error(`${path}: missing ${key}`);
  }
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) throw new Error(`${path}: unknown ${key}`);
  }
}

function validateLegacyOrder(value, path) {
  const required = new Set([
    "command_id", "session_id", "instrument_id", "side", "quantity_atoms", "order_type",
    "time_in_force", "reduce_only", "post_only", "marketable_only",
    "submitted_at_event_seq", "client_idempotency_key",
  ]);
  for (const key of required) {
    if (!(key in value)) throw new Error(`${path}: missing legacy order field ${key}`);
  }
  if (value.post_only && value.marketable_only) throw new Error(`${path}: mutually exclusive order flags`);
  if (value.order_type === "MARKET" && (value.post_only || "limit_price_atoms" in value || "stop_price_atoms" in value)) {
    throw new Error(`${path}: invalid MARKET fields`);
  }
  if (value.order_type === "LIMIT" && (!("limit_price_atoms" in value) || "stop_price_atoms" in value)) {
    throw new Error(`${path}: invalid LIMIT fields`);
  }
  if (value.order_type === "STOP_MARKET" && (!("stop_price_atoms" in value) || "limit_price_atoms" in value || value.post_only)) {
    throw new Error(`${path}: invalid STOP_MARKET fields`);
  }
  if (value.order_type === "STOP_LIMIT" && (!("stop_price_atoms" in value) || !("limit_price_atoms" in value))) {
    throw new Error(`${path}: invalid STOP_LIMIT fields`);
  }
}

function validateSubmit(value, path) {
  const required = new Set([
    "command_type", "instrument_id", "side", "quantity_atoms", "order_type", "time_in_force",
    "reduce_only", "post_only", "marketable_only",
  ]);
  const allowed = new Set([
    ...required, "limit_price_atoms", "stop_price_atoms", "price_reference", "quote_event_id",
  ]);
  exactKeys(value, required, allowed, path);
  nonemptyText(value.instrument_id, `${path}.instrument_id`);
  if (value.side !== "BUY" && value.side !== "SELL") throw new Error(`${path}.side: invalid`);
  positiveWire(value.quantity_atoms, false, `${path}.quantity_atoms`);
  if (!orderTypes.has(value.order_type)) throw new Error(`${path}.order_type: invalid`);
  if (!timeInForce.has(value.time_in_force)) throw new Error(`${path}.time_in_force: invalid`);
  booleanValue(value.reduce_only, `${path}.reduce_only`);
  booleanValue(value.post_only, `${path}.post_only`);
  booleanValue(value.marketable_only, `${path}.marketable_only`);
  if (value.post_only && value.marketable_only) throw new Error(`${path}: mutually exclusive order flags`);
  for (const field of ["limit_price_atoms", "stop_price_atoms"]) {
    if (field in value) positiveWire(value[field], true, `${path}.${field}`);
  }

  if (value.order_type === "MARKET") {
    if (value.post_only || ["limit_price_atoms", "stop_price_atoms", "price_reference", "quote_event_id"].some((key) => key in value)) {
      throw new Error(`${path}: invalid MARKET fields`);
    }
  } else if (value.order_type === "LIMIT") {
    if (!("limit_price_atoms" in value) || "stop_price_atoms" in value) throw new Error(`${path}: invalid LIMIT fields`);
  } else if (value.order_type === "STOP_MARKET") {
    if (!("stop_price_atoms" in value) || "limit_price_atoms" in value || "price_reference" in value || "quote_event_id" in value || value.post_only) {
      throw new Error(`${path}: invalid STOP_MARKET fields`);
    }
  } else if (!("stop_price_atoms" in value) || !("limit_price_atoms" in value)) {
    throw new Error(`${path}: invalid STOP_LIMIT fields`);
  }

  const hasReference = "price_reference" in value;
  const hasQuote = "quote_event_id" in value;
  if (hasReference) {
    if (!priceReferences.has(value.price_reference)) throw new Error(`${path}.price_reference: invalid`);
    if (!hasQuote || !("limit_price_atoms" in value)) throw new Error(`${path}: incomplete price reference`);
  }
  if (hasQuote) {
    nonemptyText(value.quote_event_id, `${path}.quote_event_id`);
    if (!hasReference) throw new Error(`${path}: quote_event_id requires price_reference`);
  }
}

function validateCancel(value, path) {
  const required = new Set(["command_type", "order_id"]);
  exactKeys(value, required, required, path);
  nonemptyText(value.order_id, `${path}.order_id`);
}

function validateReplace(value, path) {
  const required = new Set(["command_type", "order_id"]);
  const mutations = new Set([
    "quantity_atoms", "limit_price_atoms", "stop_price_atoms", "time_in_force", "reduce_only",
    "post_only", "marketable_only",
  ]);
  exactKeys(value, required, new Set([...required, ...mutations]), path);
  nonemptyText(value.order_id, `${path}.order_id`);
  if (![...mutations].some((key) => key in value)) throw new Error(`${path}: no replacement fields`);
  if ("quantity_atoms" in value) positiveWire(value.quantity_atoms, false, `${path}.quantity_atoms`);
  for (const field of ["limit_price_atoms", "stop_price_atoms"]) {
    if (field in value) positiveWire(value[field], true, `${path}.${field}`);
  }
  if ("time_in_force" in value && !timeInForce.has(value.time_in_force)) throw new Error(`${path}.time_in_force: invalid`);
  for (const field of ["reduce_only", "post_only", "marketable_only"]) {
    if (field in value) booleanValue(value[field], `${path}.${field}`);
  }
  if (value.post_only === true && value.marketable_only === true) throw new Error(`${path}: mutually exclusive order flags`);
}

function validateSetLeverage(value, path) {
  const required = new Set(["command_type", "leverage"]);
  exactKeys(value, required, required, path);
  if (!Number.isInteger(value.leverage) || value.leverage < 1 || value.leverage > 50) {
    throw new Error(`${path}.leverage: invalid`);
  }
}

function validateCanonicalCommand(value, path) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error(`${path}: command object required`);
  switch (value.command_type) {
    case "SUBMIT_ORDER": validateSubmit(value, path); break;
    case "CANCEL_ORDER": validateCancel(value, path); break;
    case "REPLACE_ORDER": validateReplace(value, path); break;
    case "SET_LEVERAGE": validateSetLeverage(value, path); break;
    default: throw new Error(`${path}.command_type: not canonical`);
  }
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
  if (value.command_type === "ORDER") validateLegacyOrder(value, path);
  else if (canonicalCommandTypes.has(value.command_type)) validateCanonicalCommand(value, path);

  const envelopeFields = [
    "schema_version", "command_id", "idempotency_key", "session_id", "principal_id",
    "accepted_at_ns", "logical_ts_ns", "arrival_seq", "expected_session_version", "payload",
    "payload_hash",
  ];
  if (envelopeFields.every((key) => key in value)) validateCanonicalCommand(value.payload, `${path}.payload`);
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
