use serde_json::{Map, Value};
use std::{env, fs, process};

const JS_SAFE_MAX: i128 = 9_007_199_254_740_991;
const COMMAND_TYPES: [&str; 4] = [
    "SUBMIT_ORDER",
    "CANCEL_ORDER",
    "REPLACE_ORDER",
    "SET_LEVERAGE",
];
const ORDER_TYPES: [&str; 4] = ["MARKET", "LIMIT", "STOP_MARKET", "STOP_LIMIT"];
const TIME_IN_FORCE: [&str; 3] = ["GTC", "IOC", "FOK"];
const PRICE_REFERENCES: [&str; 3] = ["BID", "ASK", "MIDPOINT"];

fn is_wire_unsigned(name: &str) -> bool {
    matches!(
        name,
        "arrival_seq"
            | "event_seq"
            | "source_sequence"
            | "canonical_tie_breaker"
            | "submitted_at_event_seq"
            | "expected_session_version"
            | "generation"
            | "trade_count"
            | "duplicates_removed"
            | "timestamp_resolution_ns"
    )
}

fn wire_int(value: &Value, signed: bool, path: &str) -> Result<i128, String> {
    let raw = value
        .as_str()
        .ok_or_else(|| format!("{path}: wire integer is not a string"))?;
    if raw == "-0"
        || raw.starts_with('+')
        || (raw.starts_with('0') && raw != "0")
        || raw.starts_with("-0")
    {
        return Err(format!("{path}: non-canonical wire integer"));
    }
    let parsed: i128 = raw
        .parse()
        .map_err(|_| format!("{path}: invalid integer"))?;
    let (low, high) = if signed {
        (i128::from(i64::MIN), i128::from(i64::MAX))
    } else {
        (0, i128::from(u64::MAX))
    };
    if parsed < low || parsed > high {
        return Err(format!("{path}: integer out of range"));
    }
    Ok(parsed)
}

fn positive_wire(value: &Value, signed: bool, path: &str) -> Result<(), String> {
    if wire_int(value, signed, path)? <= 0 {
        return Err(format!("{path}: must be positive"));
    }
    Ok(())
}

fn nonempty_text(value: Option<&Value>, path: &str) -> Result<(), String> {
    if value.and_then(Value::as_str).is_none_or(str::is_empty) {
        return Err(format!("{path}: must be non-empty"));
    }
    Ok(())
}

fn boolean(value: Option<&Value>, path: &str) -> Result<bool, String> {
    value
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{path}: must be boolean"))
}

fn exact_keys(
    map: &Map<String, Value>,
    required: &[&str],
    allowed: &[&str],
    path: &str,
) -> Result<(), String> {
    for key in required {
        if !map.contains_key(*key) {
            return Err(format!("{path}: missing {key}"));
        }
    }
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{path}: unknown {key}"));
        }
    }
    Ok(())
}

fn validate_submit(map: &Map<String, Value>, path: &str) -> Result<(), String> {
    const REQUIRED: [&str; 9] = [
        "command_type",
        "instrument_id",
        "side",
        "quantity_atoms",
        "order_type",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
    ];
    const ALLOWED: [&str; 13] = [
        "command_type",
        "instrument_id",
        "side",
        "quantity_atoms",
        "order_type",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
        "limit_price_atoms",
        "stop_price_atoms",
        "price_reference",
        "quote_event_id",
    ];
    exact_keys(map, &REQUIRED, &ALLOWED, path)?;
    nonempty_text(map.get("instrument_id"), &format!("{path}.instrument_id"))?;
    if !matches!(map.get("side").and_then(Value::as_str), Some("BUY" | "SELL")) {
        return Err(format!("{path}.side: invalid"));
    }
    positive_wire(
        map.get("quantity_atoms").expect("required checked"),
        false,
        &format!("{path}.quantity_atoms"),
    )?;
    let order_type = map
        .get("order_type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}.order_type: invalid"))?;
    if !ORDER_TYPES.contains(&order_type) {
        return Err(format!("{path}.order_type: invalid"));
    }
    let tif = map
        .get("time_in_force")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}.time_in_force: invalid"))?;
    if !TIME_IN_FORCE.contains(&tif) {
        return Err(format!("{path}.time_in_force: invalid"));
    }
    boolean(map.get("reduce_only"), &format!("{path}.reduce_only"))?;
    let post_only = boolean(map.get("post_only"), &format!("{path}.post_only"))?;
    let marketable_only = boolean(
        map.get("marketable_only"),
        &format!("{path}.marketable_only"),
    )?;
    if post_only && marketable_only {
        return Err(format!("{path}: mutually exclusive order flags"));
    }
    for field in ["limit_price_atoms", "stop_price_atoms"] {
        if let Some(value) = map.get(field) {
            positive_wire(value, true, &format!("{path}.{field}"))?;
        }
    }

    match order_type {
        "MARKET" => {
            if post_only
                || [
                    "limit_price_atoms",
                    "stop_price_atoms",
                    "price_reference",
                    "quote_event_id",
                ]
                .iter()
                .any(|field| map.contains_key(*field))
            {
                return Err(format!("{path}: invalid MARKET fields"));
            }
        }
        "LIMIT" => {
            if !map.contains_key("limit_price_atoms") || map.contains_key("stop_price_atoms") {
                return Err(format!("{path}: invalid LIMIT fields"));
            }
        }
        "STOP_MARKET" => {
            if !map.contains_key("stop_price_atoms")
                || map.contains_key("limit_price_atoms")
                || map.contains_key("price_reference")
                || map.contains_key("quote_event_id")
                || post_only
            {
                return Err(format!("{path}: invalid STOP_MARKET fields"));
            }
        }
        "STOP_LIMIT" => {
            if !map.contains_key("stop_price_atoms") || !map.contains_key("limit_price_atoms") {
                return Err(format!("{path}: invalid STOP_LIMIT fields"));
            }
        }
        _ => unreachable!(),
    }

    let has_reference = map.contains_key("price_reference");
    let has_quote = map.contains_key("quote_event_id");
    if has_reference {
        let reference = map
            .get("price_reference")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{path}.price_reference: invalid"))?;
        if !PRICE_REFERENCES.contains(&reference) {
            return Err(format!("{path}.price_reference: invalid"));
        }
        if !has_quote || !map.contains_key("limit_price_atoms") {
            return Err(format!("{path}: incomplete price reference"));
        }
    }
    if has_quote {
        nonempty_text(map.get("quote_event_id"), &format!("{path}.quote_event_id"))?;
        if !has_reference {
            return Err(format!("{path}: quote_event_id requires price_reference"));
        }
    }
    Ok(())
}

fn validate_cancel(map: &Map<String, Value>, path: &str) -> Result<(), String> {
    const KEYS: [&str; 2] = ["command_type", "order_id"];
    exact_keys(map, &KEYS, &KEYS, path)?;
    nonempty_text(map.get("order_id"), &format!("{path}.order_id"))
}

fn validate_replace(map: &Map<String, Value>, path: &str) -> Result<(), String> {
    const REQUIRED: [&str; 2] = ["command_type", "order_id"];
    const MUTATIONS: [&str; 7] = [
        "quantity_atoms",
        "limit_price_atoms",
        "stop_price_atoms",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
    ];
    const ALLOWED: [&str; 9] = [
        "command_type",
        "order_id",
        "quantity_atoms",
        "limit_price_atoms",
        "stop_price_atoms",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
    ];
    exact_keys(map, &REQUIRED, &ALLOWED, path)?;
    nonempty_text(map.get("order_id"), &format!("{path}.order_id"))?;
    if !MUTATIONS.iter().any(|field| map.contains_key(*field)) {
        return Err(format!("{path}: no replacement fields"));
    }
    if let Some(value) = map.get("quantity_atoms") {
        positive_wire(value, false, &format!("{path}.quantity_atoms"))?;
    }
    for field in ["limit_price_atoms", "stop_price_atoms"] {
        if let Some(value) = map.get(field) {
            positive_wire(value, true, &format!("{path}.{field}"))?;
        }
    }
    if let Some(value) = map.get("time_in_force") {
        let tif = value
            .as_str()
            .ok_or_else(|| format!("{path}.time_in_force: invalid"))?;
        if !TIME_IN_FORCE.contains(&tif) {
            return Err(format!("{path}.time_in_force: invalid"));
        }
    }
    for field in ["reduce_only", "post_only", "marketable_only"] {
        if map.contains_key(field) {
            boolean(map.get(field), &format!("{path}.{field}"))?;
        }
    }
    if map.get("post_only").and_then(Value::as_bool) == Some(true)
        && map.get("marketable_only").and_then(Value::as_bool) == Some(true)
    {
        return Err(format!("{path}: mutually exclusive order flags"));
    }
    Ok(())
}

fn validate_set_leverage(map: &Map<String, Value>, path: &str) -> Result<(), String> {
    const KEYS: [&str; 2] = ["command_type", "leverage"];
    exact_keys(map, &KEYS, &KEYS, path)?;
    let leverage = map
        .get("leverage")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{path}.leverage: invalid"))?;
    if !(1..=50).contains(&leverage) {
        return Err(format!("{path}.leverage: invalid"));
    }
    Ok(())
}

fn validate_canonical_command(value: &Value, path: &str) -> Result<(), String> {
    let map = value
        .as_object()
        .ok_or_else(|| format!("{path}: command object required"))?;
    match map.get("command_type").and_then(Value::as_str) {
        Some("SUBMIT_ORDER") => validate_submit(map, path),
        Some("CANCEL_ORDER") => validate_cancel(map, path),
        Some("REPLACE_ORDER") => validate_replace(map, path),
        Some("SET_LEVERAGE") => validate_set_leverage(map, path),
        _ => Err(format!("{path}.command_type: not canonical")),
    }
}

fn validate(value: &Value, path: &str) -> Result<(), String> {
    match value {
        Value::Number(number) => {
            if number.is_f64() {
                return Err(format!("{path}: floating point is forbidden"));
            }
            let n = number
                .as_i64()
                .map(i128::from)
                .or_else(|| number.as_u64().map(i128::from))
                .unwrap_or(JS_SAFE_MAX + 1);
            if n.abs() > JS_SAFE_MAX {
                return Err(format!("{path}: unsafe JSON integer"));
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                validate(item, &format!("{path}[{index}]"))?;
            }
        }
        Value::Object(map) => {
            if let Some(version) = map.get("schema_version") {
                if version.as_str() != Some("1.0.0") {
                    return Err(format!("{path}.schema_version: unsupported"));
                }
            }
            for (key, item) in map {
                let child = format!("{path}.{key}");
                if is_wire_unsigned(key) {
                    wire_int(item, false, &child)?;
                } else if key.ends_with("_atoms")
                    || key.ends_with("_minor")
                    || key.ends_with("_ppb")
                    || key.ends_with("_ns")
                {
                    let signed = !key.starts_with("qty_") && !key.ends_with("volume_atoms");
                    wire_int(item, signed, &child)?;
                }
                validate(item, &child)?;
            }
            if map.get("command_type").and_then(Value::as_str) == Some("ORDER")
                && map.get("post_only").and_then(Value::as_bool) == Some(true)
                && map.get("marketable_only").and_then(Value::as_bool) == Some(true)
            {
                return Err(format!("{path}: mutually exclusive order flags"));
            }
            if map
                .get("command_type")
                .and_then(Value::as_str)
                .is_some_and(|kind| COMMAND_TYPES.contains(&kind))
            {
                validate_canonical_command(value, path)?;
            }
            const ENVELOPE_FIELDS: [&str; 11] = [
                "schema_version",
                "command_id",
                "idempotency_key",
                "session_id",
                "principal_id",
                "accepted_at_ns",
                "logical_ts_ns",
                "arrival_seq",
                "expected_session_version",
                "payload",
                "payload_hash",
            ];
            if ENVELOPE_FIELDS.iter().all(|field| map.contains_key(*field)) {
                validate_canonical_command(
                    map.get("payload").expect("envelope payload checked"),
                    &format!("{path}.payload"),
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn main() {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: roundtrip <json-file>");
        process::exit(2);
    };
    let raw = fs::read_to_string(path).expect("read input");
    let value: Value = serde_json::from_str(&raw).expect("parse input");
    if let Err(error) = validate(&value, "$") {
        eprintln!("{error}");
        process::exit(1);
    }
    println!(
        "{}",
        serde_json::to_string(&value).expect("serialize")
    );
}
