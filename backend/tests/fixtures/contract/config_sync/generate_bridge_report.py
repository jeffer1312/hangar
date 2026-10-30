"""Captura falha parcial da ponte com diretórios de destino substituídos por arquivos."""
import json
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch
from app import skill_bridge

cases = []
for blocked in [[], ["pi"], ["kimi"], ["pi", "kimi"]]:
    with tempfile.TemporaryDirectory() as folder:
        home = Path(folder)
        repo = home / "repo"
        source = home / ".claude/skills/example"
        source.mkdir(parents=True)
        (source / "SKILL.md").write_text("fixture", encoding="utf-8")
        for name, relative in [("pi", ".pi/agent"), ("kimi", ".kimi-code")]:
            parent = home / relative
            parent.mkdir(parents=True)
            if name in blocked:
                (parent / "skills-bridge").write_text("pessoal", encoding="utf-8")
        lines = []
        with patch.object(skill_bridge, "_REPO", repo):
            stats = skill_bridge.rebuild(home, log=lines.append)
        failures = [line.strip().replace(folder, "/fixture/home") for line in lines if "rebuild falhou" in line]
        cases.append({"blocked": blocked, "stats": stats, "failures": failures})
Path(__file__).with_name("bridge-report.json").write_text(json.dumps({
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "command": "cd backend && PYTHONPATH=. uv run python tests/fixtures/contract/config_sync/generate_bridge_report.py",
    "cases": cases}, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
