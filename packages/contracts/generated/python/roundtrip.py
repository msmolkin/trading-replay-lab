#!/usr/bin/env python3
from __future__ import annotations

import json
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
sys.path.insert(0, str(SCRIPTS))
from contract_runtime import canonical_json  # noqa: E402

value = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
print(canonical_json(value))
