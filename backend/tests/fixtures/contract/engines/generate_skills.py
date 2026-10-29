"""Captura a materialização de skills com fontes inteiramente temporárias."""
import json
import subprocess
import tempfile
from pathlib import Path
from app import skill_bridge

cases = []
for scenario in ("empty", "basic", "precedence", "personal", "stale", "relative"):
    with tempfile.TemporaryDirectory() as root:
        home = Path(root)
        repo = home / "repo"
        repo.mkdir()
        skill_bridge._REPO = repo
        for folder in (".pi/agent", ".kimi-code"):
            (home / folder).mkdir(parents=True)
        if scenario != "empty":
            for folder, text in [(".claude/skills/alpha", "claude"), ("repo/skills/beta", "repo")]:
                (home / folder).mkdir(parents=True)
                (home / folder / "SKILL.md").write_text(text)
        if scenario == "precedence":
            for folder in ("repo/skills/alpha", ".claude/plugins/cache/a/b/2.2/skills/version", ".claude/plugins/cache/a/b/2.10/skills/version"):
                (home / folder).mkdir(parents=True)
                (home / folder / "SKILL.md").write_text("fixture")
        for folder in (".pi/agent/skills-bridge", ".kimi-code/skills-bridge"):
            bridge = home / folder
            bridge.mkdir()
            if scenario == "personal":
                (bridge / "alpha").mkdir()
                (bridge / "beta").symlink_to(home / "personal")
            if scenario == "stale":
                (bridge / "gone").symlink_to(home / ".claude/skills/gone")
                (bridge / "alpha").symlink_to(home / ".claude/skills/missing")
            if scenario == "relative":
                import os
                (bridge / "alpha").symlink_to(os.path.relpath(home / ".claude/skills/alpha", bridge))
        result = skill_bridge.rebuild(home, log=lambda *_:None)
        again = skill_bridge.rebuild(home, log=lambda *_:None)
        links = {str(p.relative_to(home)):str(p.readlink()) for p in home.rglob("*") if p.is_symlink()}
        cases.append(json.loads(json.dumps({"scenario":scenario,"expected":result,"again":again,"links":links}).replace(root,"$ROOT")))
output = {"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_skills.py","cases":cases}
Path(__file__).with_name("skills.json").write_text(json.dumps(output,ensure_ascii=False,indent=2)+"\n")
