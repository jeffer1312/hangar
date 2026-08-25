"""preview_hook.py: o publicador de previa do Claude Code (MessageDisplay + Stop).

Payloads copiados do shape REAL medido em 17/08/2026 (claude 2.1.233, TUI interativo): deltas
INCREMENTAIS com index crescente e final no ultimo — nao o que a doc sugere, o que o hook manda.
"""
import json, os, subprocess, sys
from pathlib import Path

HOOK = str(Path(__file__).resolve().parent.parent / "hooks" / "preview_hook.py")


def _run(payload: dict, config_dir: Path) -> None:
    env = {**os.environ, "CLAUDE_CONFIG_DIR": str(config_dir)}
    subprocess.run([sys.executable, HOOK], input=json.dumps(payload).encode(),
                   env=env, check=True, timeout=10)


def _sidecar(config_dir: Path, stem: str) -> dict:
    return json.loads((config_dir / ".hangar-preview" / f"{stem}.json").read_text())


def _md(delta: str, index: int, *, mid: str = "m1", tp: str = "/x/projects/p/abc.jsonl",
        final: bool = False, agent_id: str | None = None) -> dict:
    o = {"hook_event_name": "MessageDisplay", "session_id": "sess-cmdline", "transcript_path": tp,
         "message_id": mid, "index": index, "final": final, "delta": delta}
    if agent_id:
        o["agent_id"] = agent_id
    return o


def test_primeiro_delta_publica_e_chave_e_o_stem_do_transcript(tmp_path):
    _run(_md("# Titulo\n\n", 0), tmp_path)
    # chave = stem do jsonl (abc), NUNCA o session_id do cmdline — sessao retomada diverge
    o = _sidecar(tmp_path, "abc")
    assert o["text"] == "# Titulo\n\n"
    assert isinstance(o["ts"], float)


def test_deltas_incrementais_acumulam_na_mesma_mensagem(tmp_path):
    _run(_md("# Titulo\n\n", 0), tmp_path)
    _run(_md("Primeiro paragrafo.", 1), tmp_path)
    _run(_md(" Segundo.", 2, final=True), tmp_path)
    assert _sidecar(tmp_path, "abc")["text"] == "# Titulo\n\nPrimeiro paragrafo. Segundo."


def test_mensagem_nova_substitui_a_anterior_nao_soma(tmp_path):
    # Contrato do preview: publica o ULTIMO bloco, nao a soma do turno — a soma faria o
    # preview_is_committed engolir tudo como prefixo do commitado.
    _run(_md("Mensagem um.", 0, mid="m1"), tmp_path)
    _run(_md("Mensagem dois.", 0, mid="m2"), tmp_path)
    assert _sidecar(tmp_path, "abc")["text"] == "Mensagem dois."


def test_stop_zera_a_previa(tmp_path):
    _run(_md("Em voo.", 0), tmp_path)
    _run({"hook_event_name": "Stop", "session_id": "s",
          "transcript_path": "/x/projects/p/abc.jsonl"}, tmp_path)
    assert _sidecar(tmp_path, "abc")["text"] == ""


def test_texto_de_subagente_nao_vira_previa(tmp_path):
    _run(_md("prosa de subagente", 0, agent_id="ag-1"), tmp_path)
    assert not (tmp_path / ".hangar-preview" / "abc.json").exists()


def test_continuacao_sem_sidecar_anterior_publica_so_o_delta(tmp_path):
    # sidecar apagado no meio (limpeza, /clear): melhor previa curta que crash/nada
    _run(_md("cauda da mensagem", 3), tmp_path)
    assert _sidecar(tmp_path, "abc")["text"] == "cauda da mensagem"


def test_sem_transcript_path_cai_no_session_id(tmp_path):
    _run(_md("oi", 0, tp=""), tmp_path)
    assert _sidecar(tmp_path, "sess-cmdline")["text"] == "oi"


def test_session_id_hostil_nao_escapa_do_diretorio(tmp_path):
    # stem vira nome de arquivo: um session_id com `../` tem que ficar preso no sidecar dir
    _run(_md("oi", 0, tp=""), tmp_path)  # garante o dir base
    payload = _md("malicioso", 0, tp="")
    payload["session_id"] = "../../fora"
    _run(payload, tmp_path)
    assert not (tmp_path.parent / "fora.json").exists()
    assert (tmp_path / ".hangar-preview" / "fora.json").exists()


def test_delta_retardatario_depois_do_stop_nao_ressuscita_previa(tmp_path):
    # Stop fechou o turno ("" no sidecar); um MessageDisplay atrasado (index > 0, outra msg)
    # nao pode republicar um rabo de texto como se estivesse em voo
    _run({"hook_event_name": "Stop", "session_id": "s",
          "transcript_path": "/x/projects/p/abc.jsonl"}, tmp_path)
    _run(_md("rabo atrasado", 3, mid="m-velha"), tmp_path)
    assert _sidecar(tmp_path, "abc")["text"] == ""


def test_evento_desconhecido_nao_escreve_nada(tmp_path):
    _run({"hook_event_name": "Notification", "session_id": "s",
          "transcript_path": "/x/projects/p/abc.jsonl"}, tmp_path)
    assert not (tmp_path / ".hangar-preview").exists()


def test_stdin_quebrado_sai_zero_sem_escrever(tmp_path):
    env = {**os.environ, "CLAUDE_CONFIG_DIR": str(tmp_path)}
    r = subprocess.run([sys.executable, HOOK], input=b"{nao e json", env=env, timeout=10)
    assert r.returncode == 0
    assert not (tmp_path / ".hangar-preview").exists()
