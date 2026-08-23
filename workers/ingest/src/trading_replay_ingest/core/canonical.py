"""Canonical JSON helpers for ingestion metadata and records."""

from json import dumps


type JsonValue = bool | int | str | list[JsonValue] | dict[str, JsonValue] | None


def require_json_value(value: object) -> JsonValue:
    """Return a canonical JSON value or fail closed on floats/unknown objects."""
    if value is None or isinstance(value, bool | int | str):
        return value
    if isinstance(value, float):
        raise TypeError("floating-point values are forbidden in canonical ingestion output")
    if isinstance(value, list | tuple):
        return [require_json_value(item) for item in value]
    if isinstance(value, dict):
        result: dict[str, JsonValue] = {}
        for key, item in value.items():
            if not isinstance(key, str):
                raise TypeError("canonical JSON object keys must be strings")
            result[key] = require_json_value(item)
        return result
    raise TypeError(f"unsupported canonical JSON value: {type(value).__name__}")


def canonical_bytes(value: object) -> bytes:
    """Serialize canonical JSON deterministically as UTF-8 without insignificant space."""
    normalized = require_json_value(value)
    text = dumps(
        normalized,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
    )
    return text.encode("utf-8")
