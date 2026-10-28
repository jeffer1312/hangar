"""Captura os desempates no checkout Python anterior à migração."""
import json
import os
from pathlib import Path
import sys

reference = os.environ.get("HANGAR_ORQ_REFERENCE_ROOT")
if not reference:
    raise SystemExit("informe HANGAR_ORQ_REFERENCE_ROOT com o checkout Python de origem")
sys.path.insert(0, str(Path(reference) / "backend"))
from app.orq_papeis import Papel, casar_viva
from app.models import SessionInfo

cases = []
for activities in ((1.0, 1.0), (None, None), (-0.0, 0.0)):
    rows = [{"name": "worker-a", "last_activity": activities[0]},
            {"name": "worker-b", "last_activity": activities[1]}]
    expected = casar_viva(Papel("executor", "worker-*", "claude", "default", "", ""),
                         [SessionInfo(**row) for row in rows])
    cases.append({"rows": rows, "expected": expected})
Path(__file__).with_name("role-live-ties.json").write_text(json.dumps({
    "source_commit": "a0860f26b6e3f611806e780acf87588f6065b328",
    "command": "HANGAR_ORQ_REFERENCE_ROOT=<checkout-origem> python backend/tests/fixtures/contract/orq/generate_role_live.py",
    "cases": cases}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
