"""Captura precisão, arredondamento e limites do horário original da integração."""
import ast
import argparse
import base64
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
def capture(function, inputs):
    cases = []
    for value in inputs:
        row = {"value": repr(value)}
        try:
            row["result"] = function(value)
        except Exception as error:
            row.update(error=type(error).__name__, message=str(error))
        cases.append(row)
    return cases

parser = argparse.ArgumentParser()
parser.add_argument("--windows-host")
args = parser.parse_args()
if args.windows_host:
    own = ast.parse(Path(__file__).read_text())
    capture_source = ast.unparse(next(node for node in own.body if isinstance(node, ast.FunctionDef) and node.name == "capture"))
    script = "from datetime import datetime, timezone\nimport json\n" + ast.unparse(tree) + "\n" + capture_source
    script += f"\nprint(json.dumps(capture(_iso, [float(value) for value in {[repr(value) for value in values]!r}])))\n"
    payload = base64.b64encode(script.encode()).decode()
    powershell = f"python -c \"import base64; exec(base64.b64decode('{payload}'))\""
    encoded = base64.b64encode(powershell.encode("utf-16le")).decode()
    output = subprocess.check_output(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", args.windows_host,
                                      "powershell -NoProfile -EncodedCommand " + encoded], text=True, timeout=30)
    cases = json.loads(output)
else:
    cases = capture(namespace["_iso"], values)
Path(__file__).with_name("timestamps-windows.json" if args.windows_host else "timestamps.json").write_text(json.dumps({
    "source_commit": COMMIT,
    "command": "cd backend && uv run python tests/fixtures/contract/codex-integration/generate_timestamps.py" + (" --windows-host <host>" if args.windows_host else ""),
    "cases": cases,
}, ensure_ascii=True, indent=2) + "\n")
print(len(cases))
