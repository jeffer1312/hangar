import ast
import json
import logging
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from app import diag, diag_logging, log_paths


@pytest.fixture
def ambiente(tmp_path):
    return {**os.environ, "HOME": str(tmp_path / "home"), "USERPROFILE": str(tmp_path / "home"),
            "LOCALAPPDATA": str(tmp_path / "local"), "CLAUDE_CONFIG_DIR": str(tmp_path / "conta")}


@pytest.mark.parametrize("plataforma,local", [("posix", True), ("nt", True), ("nt", False)])
@pytest.mark.parametrize("hook,arquivo", [("guard_tmux.py", "guard_tmux-falhas.log"),
                                        ("kimi_state_hook.py", "kimi_hook_error.log")])
def test_hook_falha_grava_na_pasta_da_maquina(tmp_path, ambiente, plataforma, local, hook, arquivo):
    path = Path(__file__).resolve().parents[1] / "hooks" / hook
    if not local:
        ambiente.pop("LOCALAPPDATA")
    # Só o subprocesso simula a plataforma; pathlib/pytest do processo pai ficam intactos.
    launcher = (
        "import os,sys,json,re,shlex,time,traceback; "
        "source=open(sys.argv[2],encoding='utf-8').read(); "
        "os.name=sys.argv[1]; exec(compile(source,sys.argv[2],'exec'))"
    )
    payload = json.dumps({"tool_name": "Bash", "tool_input": {"command": {"invalido": True}}})
    result = subprocess.run([sys.executable, "-c", launcher, plataforma, str(path)], env=ambiente,
                            input=payload if hook == "guard_tmux.py" else "json invalido",
                            capture_output=True, text=True, timeout=10)
    assert result.returncode == 0 and result.stdout == ""
    if plataforma == "posix":
        root = tmp_path / "home" / ".hangar"
    else:
        root = (tmp_path / "local" if local else tmp_path / "home" / "AppData" / "Local") / "hangar"
    log = root / "logs" / "privado" / arquivo
    assert log.read_text(encoding="utf-8")
    assert not (tmp_path / "conta" / arquivo).exists()
    if os.name == "posix":
        assert log.stat().st_mode & 0o777 == 0o600


@pytest.mark.skipif(shutil.which("bash") is None, reason="bash não instalado")
@pytest.mark.parametrize("windows,local", [(False, True), (True, True), (True, False)])
def test_restore_vazio_grava_log_sem_tocar_tmux(tmp_path, ambiente, windows, local):
    script = Path(__file__).resolve().parents[2] / "scripts" / "tmux-claude-resume.sh"
    resurrect = tmp_path / "resurrect"
    resurrect.mkdir()
    (resurrect / "claude-sessions.tsv").write_text("")
    ambiente["TMUX_RESURRECT_DIR"] = str(resurrect)
    ambiente["OS"] = "Windows_NT" if windows else "Linux"
    if not local:
        ambiente.pop("LOCALAPPDATA")
    result = subprocess.run(["bash", str(script), "restore"], env=ambiente,
                            capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    root = tmp_path / "home" / ".hangar"
    if windows:
        root = (tmp_path / "local" if local else tmp_path / "home" / "AppData" / "Local") / "hangar"
    assert "restore: as sessões do Hangar voltam pelo servidor" in (root / "logs" / "privado" / "claude-resume.log").read_text()
    assert not (resurrect / "claude-resume.log").exists()
    assert (resurrect / "claude-sessions.tsv").read_text() == ""
