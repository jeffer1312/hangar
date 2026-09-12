import ast
import json
import logging
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from app import atualizar, diag, diag_logging, log_paths


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
    assert "restore: start" in (root / "logs" / "privado" / "claude-resume.log").read_text()
    assert not (resurrect / "claude-resume.log").exists()
    assert (resurrect / "claude-sessions.tsv").read_text() == ""


def test_motor_atualizador_instala_log_central_antes_de_executar(tmp_path, monkeypatch, ambiente):
    for key in ("HOME", "USERPROFILE", "LOCALAPPDATA"):
        monkeypatch.setenv(key, ambiente[key])
    root = tmp_path / "logs"
    monkeypatch.setattr(log_paths, "base", lambda: root)
    monkeypatch.setattr(diag, "_base", lambda: root / "diario")
    tree = ast.parse(Path(atualizar.__file__).read_text(encoding="utf-8"))
    main = next(node for node in tree.body if isinstance(node, ast.If)
                and ast.unparse(node.test) == "__name__ == '__main__'")
    loggers = [logging.getLogger(name) for name in ("hangar", "app", "uvicorn.error", "asyncio")]
    saved = [(logger, logger.handlers[:], logger.level) for logger in loggers]
    called = []

    def executar(porta):
        called.append(porta)
        logging.getLogger("hangar.atualizar").error("falha privada: token-secreto")

    try:
        for logger in loggers:
            logger.handlers = []
        exec(compile(ast.Module(body=main.body, type_ignores=[]), atualizar.__file__, "exec"),
             {"logging": logging, "sys": type("Args", (), {"argv": ["atualizar", "9000"]}),
              "executar": executar})
        assert called == [9000]
        assert "token-secreto" in (root / "privado" / "atualizacao.log").read_text()
        raw = diag.caminho_do_dia().read_text()
        assert "token-secreto" not in raw
        assert any(row["evento"] == "backend.erro" for row in map(json.loads, raw.splitlines()))
    finally:
        created = {handler for logger in loggers for handler in logger.handlers
                   if isinstance(handler, (diag_logging.PrivateLog, diag_logging.DiaryHandler))}
        for logger, handlers, level in saved:
            logger.handlers = handlers
            logger.setLevel(level)
        for handler in created:
            handler.close()
