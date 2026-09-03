"""pair_hook.py: SessionStart devolve o protocolo do grupo como additionalContext."""
import json, os, subprocess, sys
from pathlib import Path

HOOK = str(Path(__file__).resolve().parent.parent / "hooks" / "pair_hook.py")


def _run(pair_dir: Path, env_extra: dict) -> str:
    env = {k: v for k, v in os.environ.items() if k not in ("TMUX_PANE", "CP_SESSION_NAME")}
    env.update(env_extra)
    return subprocess.run([sys.executable, HOOK, str(pair_dir)],
                          input=json.dumps({"hook_event_name": "SessionStart", "source": "clear"}).encode(),
                          env=env, capture_output=True, timeout=10).stdout.decode()


def _fake_tmux(tmp_path: Path, panes: str, has_session_rc: int = 0) -> Path:
    # Um `tmux` de mentira no PATH: list-panes imprime a tabela dada; has-session sai com o rc dado.
    d = tmp_path / "bin"; d.mkdir()
    sh = d / "tmux"
    sh.write_text("#!/bin/sh\n"
                  f"if [ \"$1\" = list-panes ]; then printf '%s' '{panes}'; exit 0; fi\n"
                  f"if [ \"$1\" = has-session ]; then exit {has_session_rc}; fi\nexit 1\n")
    sh.chmod(0o755)
    return d


def test_sem_grupo_nao_imprime_nada(tmp_path):
    bin_ = _fake_tmux(tmp_path, "%3\tapi\n")
    out = _run(tmp_path, {"TMUX_PANE": "%3", "PATH": f"{bin_}:{os.environ['PATH']}"})
    assert out == ""


def test_com_grupo_devolve_additional_context(tmp_path):
    (tmp_path / "api.json").write_text(json.dumps({"peers": ["front"], "task": "PM-9", "gid": "g1"}))
    bin_ = _fake_tmux(tmp_path, "%3\tapi\n%4\tfront\n")
    out = _run(tmp_path, {"TMUX_PANE": "%3", "PATH": f"{bin_}:{os.environ['PATH']}"})
    d = json.loads(out)
    ctx = d["hookSpecificOutput"]["additionalContext"]
    assert d["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    assert ctx.startswith("[de: hangar] GRUPO DE TRABALHO ATIVO: você ('api') trabalha junto com 'front' na tarefa: PM-9.")
    assert str(tmp_path / "grupo-g1.md") in ctx


def test_gid_vazio_nao_cita_contrato(tmp_path):
    (tmp_path / "api.json").write_text(json.dumps({"peers": ["front"], "task": "", "gid": ""}))
    bin_ = _fake_tmux(tmp_path, "%3\tapi\n")
    out = _run(tmp_path, {"TMUX_PANE": "%3", "PATH": f"{bin_}:{os.environ['PATH']}"})
    assert "Contrato/decisões" not in json.loads(out)["hookSpecificOutput"]["additionalContext"]


def test_pane_ambiguo_cai_no_carimbo(tmp_path):
    # psmux numera pane por sessão: %1 em duas sessões -> "não sei" -> CP_SESSION_NAME (se viva).
    (tmp_path / "b.json").write_text(json.dumps({"peers": ["c"], "task": "", "gid": "g2"}))
    bin_ = _fake_tmux(tmp_path, "%1\ta\n%1\tb\n")
    out = _run(tmp_path, {"TMUX_PANE": "%1", "CP_SESSION_NAME": "b", "PATH": f"{bin_}:{os.environ['PATH']}"})
    assert "você ('b')" in json.loads(out)["hookSpecificOutput"]["additionalContext"]


def test_sidecar_torto_nao_trava(tmp_path):
    (tmp_path / "api.json").write_text("{nao é json")
    bin_ = _fake_tmux(tmp_path, "%3\tapi\n")
    out = _run(tmp_path, {"TMUX_PANE": "%3", "PATH": f"{bin_}:{os.environ['PATH']}"})
    assert out == ""
