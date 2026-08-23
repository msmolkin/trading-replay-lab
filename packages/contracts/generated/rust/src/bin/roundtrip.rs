use serde_json::Value;
use std::{env, fs, process};

const JS_SAFE_MAX: i128 = 9_007_199_254_740_991;

fn is_wire_unsigned(name: &str) -> bool {
    matches!(
        name,
        "arrival_seq" | "event_seq" | "source_sequence" | "canonical_tie_breaker"
            | "submitted_at_event_seq" | "expected_session_version" | "generation"
            | "trade_count" | "duplicates_removed" | "timestamp_resolution_ns"
    )
}

fn wire_int(value: &Value, signed: bool, path: &str) -> Result<(), String> {
    let raw = value.as_str().ok_or_else(|| format!("{path}: wire integer is not a string"))?;
    if raw == "-0" || raw.starts_with('+') || (raw.starts_with('0') && raw != "0") || raw.starts_with("-0") {
        return Err(format!("{path}: non-canonical wire integer"));
    }
    let parsed: i128 = raw.parse().map_err(|_| format!("{path}: invalid integer"))?;
    let (low, high) = if signed { (i64::MIN as i128, i64::MAX as i128) } else { (0, u64::MAX as i128) };
    if parsed < low || parsed > high {
        return Err(format!("{path}: integer out of range"));
    }
    Ok(())
}

fn validate(value: &Value, path: &str) -> Result<(), String> {
    match value {
        Value::Number(number) => {
            if number.is_f64() {
                return Err(format!("{path}: floating point is forbidden"));
            }
            let n = number.as_i64().map(i128::from).or_else(|| number.as_u64().map(i128::from)).unwrap_or(JS_SAFE_MAX + 1);
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
                } else if key.ends_with("_atoms") || key.ends_with("_minor") || key.ends_with("_ppb") || key.ends_with("_ns") {
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
    println!("{}", serde_json::to_string(&value).expect("serialize"));
}
