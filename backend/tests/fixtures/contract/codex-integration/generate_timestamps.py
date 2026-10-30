"""Captura precisão, arredondamento e limites do horário original da integração."""
import ast
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess

COMMIT = "8a3ce81c1"
source = subprocess.check_output(["git", "show", f"{COMMIT}:backend/app/codex_integracao.py"], text=True)
tree = ast.parse(source)
tree.body = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "_iso"]
namespace = {"datetime": datetime, "timezone": timezone}
exec(compile(tree, "codex_integracao_original.py", "exec"), namespace)
values = [0.0, -0.0, 100.0, 100.001, 100.000001, 100.1234567,
          0.0000005, 0.0000015, 0.0000025, 100.9999995, 100.9999999,
          -0.0000005, -0.0000015, -100.001, -100.9999995,
          -62135596800.0, -62135596800.00001, 253402300799.99997,
          253402300800.0, 1e20, -1e20, float("nan"), float("inf"), -float("inf")]
cases = []
for value in values:
    row = {"value": repr(value)}
    try:
        row["result"] = namespace["_iso"](value)
    except Exception as error:
        row.update(error=type(error).__name__, message=str(error))
    cases.append(row)
Path(__file__).with_name("timestamps.json").write_text(json.dumps({
    "source_commit": COMMIT,
    "command": "cd backend && uv run python tests/fixtures/contract/codex-integration/generate_timestamps.py",
    "cases": cases,
}, ensure_ascii=True, indent=2) + "\n")
print(len(cases))
