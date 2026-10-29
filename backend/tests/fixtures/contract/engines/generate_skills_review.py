"""Captura falha de leitura e configuração de consumo da ponte em ambiente isolado."""
import json
import os
import subprocess
import tempfile
from pathlib import Path
from app import skill_bridge as module

with tempfile.TemporaryDirectory() as directory:
    home = Path(directory)
    os.environ.update(HOME=directory, SKILL_HOME=directory)
    module._REPO = home / "repo"
    for name in (".claude/skills/protected", "repo/skills/visible", ".pi/agent/skills-bridge", ".kimi-code"):
        (home / name).mkdir(parents=True)
    for name in (".claude/skills/protected/SKILL.md", "repo/skills/visible/SKILL.md"):
        (home / name).write_text("fixture")
    link = home / ".pi/agent/skills-bridge/protected"
    link.symlink_to(home / ".claude/skills/protected")
    source = home / ".claude/skills"
    source.chmod(0)
    try:
        module.rebuild(home, log=lambda _: None)
        failure = None
    except OSError as error:
        failure = type(error).__name__
    finally:
        source.chmod(0o700)
    unreadable = {"error":failure,"link_preserved":link.is_symlink()}
    cases = []
    for target, relative, file, key, check in (("pi",".pi/agent","settings.json","skills",module._config_ok_pi),("kimi",".kimi-code","config.toml","extra_skill_dirs",module._config_ok_kimi)):
        for prefix in (None,"~","$HOME","${SKILL_HOME}"):
            entries = [] if prefix is None else [f"{prefix}/{relative}/unused/../skills-bridge"]
            raw = json.dumps({key:entries}) if target == "pi" else f"{key} = {json.dumps(entries)}\n"
            config = home / relative / file
            config.write_text(raw)
            logs = []
            module.rebuild(home, log=logs.append)
            cases.append({"target":target,"prefix":prefix,"configured":check(home,home/relative/"skills-bridge"),"warned":any(f"não está na config do {target}" in line for line in logs),"unchanged":config.read_text()==raw})
Path(__file__).with_name("skills-review.json").write_text(json.dumps({"source_commit":subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip(),"command":"PYTHONPATH=backend python backend/tests/fixtures/contract/engines/generate_skills_review.py","unreadable":unreadable,"cases":cases},ensure_ascii=False,indent=2)+"\n")
