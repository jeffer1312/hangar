"""Painel de saúde dos harnesses (app/harness_saude.py): checagem lê, conserto reusa o instalador."""
import json
from pathlib import Path

from app import harness_saude as h


def test_extensoes_faltando_e_o_conserto_liga(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    (repo / "scripts" / "pi").mkdir(parents=True)
    for nome in h._EXTENSOES_PI:
        (repo / "scripts" / "pi" / f"{nome}.ts").write_text("")
    monkeypatch.setattr(h, "_REPO", repo)
    monkeypatch.setenv("HOME", str(tmp_path))
    raiz = tmp_path / ".pi" / "agent"
    (raiz / "extensions").mkdir(parents=True)
    # Arquivo real do usuário com o mesmo nome: é dele, não é falta nem vira symlink.
    (raiz / "extensions" / "claude-todo.ts").write_text("meu")
    item = h._extensoes("pi")
    assert item["ok"] is False and item["codigo"] == "faltam" and item["conserto"] == "extensoes:pi"
    assert "claude-todo" not in item["params"]["lista"] and "hangar-state" in item["params"]["lista"]
    assert h.consertar(item["conserto"]) == f"{len(h._EXTENSOES_PI) - 1} extensões ligadas"
    assert h._extensoes("pi")["ok"] is True
    assert (raiz / "extensions" / "claude-todo.ts").read_text() == "meu"


def test_hooks_do_claude_faltando_apontam_o_conserto(tmp_path):
    (tmp_path / "settings.json").write_text(json.dumps({"hooks": {"Stop": [{"hooks": [{"command": "x state_hook.py"}]}]}}))
    item = h._hooks_claude(tmp_path)
    assert item["ok"] is False and item["conserto"] == "hooks-claude"
    assert "askq_capture.py" in item["params"]["lista"] and "state_hook.py" not in item["params"]["lista"]
    (tmp_path / "settings.json").write_text("{quebrado")
    assert h._hooks_claude(tmp_path)["ok"] is None


def test_conserto_desconhecido_ou_caminho_arbitrario_falha_alto():
    for id_ in ("nada", "extensoes:/tmp/qualquer"):
        try:
            h.consertar(id_)
        except ValueError:
            continue
        raise AssertionError(f"{id_} não pode virar no-op nem escrever fora dos agentes")
