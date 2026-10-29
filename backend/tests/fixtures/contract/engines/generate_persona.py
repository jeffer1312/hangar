"""Captura o instalador antigo de persona em HOME temporário, sem tocar contas reais."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

repo = Path(__file__).resolve().parents[5]
source = (repo / "scripts/install-skills-bridge.sh").read_text().split("python3 - <<'PYEOF'\n", 1)[1].split("\nPYEOF", 1)[0]
cases = []
for scenario in ("empty", "both", "personal", "link", "packages", "missing_persona"):
    with tempfile.TemporaryDirectory() as temporary:
        home = Path(temporary)
        (home / ".claude/plugins").mkdir(parents=True)
        installed = {"plugins": {}}
        if scenario != "missing_persona":
            (home / ".claude/CLAUDE.md").write_text("fixture")
        if scenario != "empty":
            for path in (".pi/agent", ".kimi-code"):
                (home / path).mkdir(parents=True)
            for path in (".pi/agent/CLAUDE.md", ".kimi-code/AGENTS.md"):
                if scenario == "personal": (home / path).write_text("pessoal")
                if scenario == "link": (home / path).symlink_to(home / "anterior")
        if scenario == "packages":
            installed["plugins"] = {"superpowers@claude-plugins-official": [{"installPath": str(home / "plugin-1")}, {"installPath": str(home / "plugin-2")}], "ponytail@ponytail": [{"installPath": str(home / "ponytail")}]}
        (home / ".claude/plugins/installed_plugins.json").write_text(json.dumps(installed))
        result = subprocess.run([sys.executable, "-c", source], env={**os.environ, "HOME": str(home)}, text=True, capture_output=True, check=True)
        links = {str(path.relative_to(home)): os.readlink(path) for path in home.rglob("*") if path.is_symlink()}
        cases.append(json.loads(json.dumps({"scenario": scenario, "installed": installed, "stdout": result.stdout, "links": links}).replace(temporary, "$ROOT")))
Path(__file__).with_name("persona.json").write_text(json.dumps({"source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip(), "command": "python backend/tests/fixtures/contract/engines/generate_persona.py", "cases": cases}, ensure_ascii=False, indent=2) + "\n")
