"""Executa o código original com PureWindowsPath; prova lexical, sem simular E/S Windows."""
import json
from pathlib import Path, PureWindowsPath
import subprocess
import sys
import types

SOURCE = "fc1ed83a37fe22c126328f3717ccd6b37db17e10"
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
source = subprocess.check_output(
    ["git", "show", f"{SOURCE}:backend/app/config_sync_paths.py"], cwd=ROOT, text=True
)
module = types.ModuleType("original_config_sync_paths")
sys.modules[module.__name__] = module
exec(compile(source, "original_config_sync_paths.py", "exec"), module.__dict__)
module.Path = PureWindowsPath
rows = []
cases = json.loads((HERE / "helpers.json").read_text(encoding="utf-8"))["paths"]
for case in cases:
    roots = module.Roots(**case["roots"])
    text = case["input"]
    row = {"roots": case["roots"], "input": text,
           "canonical": module.canonicalize(text, roots),
           "resolved": module.resolve(text, roots), "marked": module.marked_paths(text)}
    if row != case:
        rows.append(row)
result = {"source_commit": SOURCE,
          "command": "python3 backend/tests/fixtures/contract/config_sync/generate_windows_paths.py",
          "scope": "Código Python original com pathlib.PureWindowsPath; somente operações lexicais, sem E/S Windows",
          "cases_checked": len(cases), "paths": rows}
(HERE / "helpers-windows-paths.json").write_text(
    json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
)
