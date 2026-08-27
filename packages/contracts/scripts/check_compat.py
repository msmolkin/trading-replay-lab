#!/usr/bin/env python3
"""Fail closed on broken refs, unsafe numerics, unsupported versions, and fixture regressions."""

from __future__ import annotations

import json
import runpy
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
SCHEMA_DIR = ROOT / "schemas/v1"
SCRIPTS = ROOT / "packages/contracts/scripts"
sys.path.insert(0, str(SCRIPTS))
from contract_runtime import ContractError, load_and_validate  # noqa: E402


def walk(value: Any, path: str = "$") -> None:
    if isinstance(value, float):
        raise RuntimeError(f"{path}: floating-point literal in schema")
    if isinstance(value, list):
        for index, item in enumerate(value):
            walk(item, f"{path}[{index}]")
        return
    if not isinstance(value, dict):
        return
    if value.get("type") == "number":
        raise RuntimeError(f"{path}: canonical schema declares a floating-point number")
    for key, item in value.items():
        walk(item, f"{path}.{key}")


def resolve_pointer(document: Any, fragment: str, source: str) -> None:
    if not fragment:
        return
    if not fragment.startswith("/"):
        raise RuntimeError(f"{source}: unsupported ref fragment #{fragment}")
    node = document
    for raw_part in fragment.lstrip("/").split("/"):
        part = raw_part.replace("~1", "/").replace("~0", "~")
        if not isinstance(node, dict) or part not in node:
            raise RuntimeError(f"{source}: unresolved ref fragment #{fragment}")
        node = node[part]


def check_refs(path: Path, document: Any, documents: dict[str, Any]) -> None:
    def visit(value: Any) -> None:
        if isinstance(value, list):
            for item in value:
                visit(item)
            return
        if not isinstance(value, dict):
            return
        ref = value.get("$ref")
        if isinstance(ref, str):
            target_name, _, fragment = ref.partition("#")
            target = document if not target_name else documents.get(target_name)
            if target is None:
                raise RuntimeError(f"{path.name}: unresolved schema {target_name!r}")
            resolve_pointer(target, fragment, path.name)
        for item in value.values():
            visit(item)

    visit(document)


def check_python_model_surface() -> None:
    namespace = runpy.run_path(str(ROOT / "packages/contracts/generated/python/models.py"))
    required = {
        "InstrumentDefinition",
        "OrderCommand",
        "SubmitOrderCommand",
        "SetLeverageCommand",
        "CancelOrderCommand",
        "ReplaceOrderCommand",
        "CommandPayload",
        "CommandEnvelope",
        "MarketEvent",
        "Gap",
        "DataCapabilities",
        "DatasetManifest",
        "DomainEvent",
        "SessionVisibility",
        "StateHash",
        "ResultMetrics",
        "ResultBundle",
    }
    missing = sorted(required.difference(namespace))
    if missing:
        raise RuntimeError(f"generated Python model surface is incomplete: {missing}")


def main() -> int:
    schema_paths = sorted(SCHEMA_DIR.glob("*.schema.json"))
    documents = {path.name: json.loads(path.read_text(encoding="utf-8")) for path in schema_paths}
    for path, document in [(p, documents[p.name]) for p in schema_paths]:
        walk(document, path.name)
        check_refs(path, document, documents)

    common = documents["common.schema.json"]["$defs"]
    for name in ("WireInt64", "WireUInt64", "PositiveWireUInt64"):
        if common[name].get("type") != "string":
            raise RuntimeError(f"{name} must remain a JSON string")
    if common["SchemaVersion"].get("const") != "1.0.0":
        raise RuntimeError("v1 schemas must reject unsupported majors")

    valid = sorted((SCHEMA_DIR / "examples/valid").glob("*.json"))
    invalid = sorted((SCHEMA_DIR / "examples/invalid").glob("*.json"))
    for path in valid:
        load_and_validate(path)
    for path in invalid:
        try:
            load_and_validate(path)
        except ContractError:
            pass
        else:
            raise RuntimeError(f"invalid fixture unexpectedly accepted: {path.name}")

    check_python_model_surface()
    subprocess.run([sys.executable, str(SCRIPTS / "generate.py"), "--check"], cwd=ROOT, check=True)
    subprocess.run([sys.executable, str(SCRIPTS / "roundtrip.py")], cwd=ROOT, check=True)
    print("Schema compatibility: refs, numerics, versions, fixtures, generated models, and identity are valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
