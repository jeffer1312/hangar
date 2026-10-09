"""Janela escondida do Claude que o Rust opera por `ClaudeWindows` (app/claude_window.py).

O código colado vai por stdin e nunca chega à janela nem ao processo com caractere de controle,
nem à resolução da conta pela ponte. A renovação só abre o `claude` em pasta confiada da conta
(`hasTrustDialogAccepted` literalmente `true`) que ainda existe.
"""
import json
from pathlib import Path

import pytest

from app import claude_window


@pytest.mark.parametrize("code", ["synthetic\nsecond-command", "synthetic\rsecond-command", "synthetic\x00tail"])
def test_protected_code_rejects_control_characters_before_io(monkeypatch, code):
    calls = []
    monkeypatch.setattr(claude_window.tmux, "paste_via_clipboard", lambda *args: calls.append("clipboard") or True)
    monkeypatch.setattr(claude_window.tmux, "send_keys", lambda *args: calls.append("keys") or True)
    monkeypatch.setattr(claude_window.tmux, "_run", lambda *args, **kwargs: calls.append("process"))
    with pytest.raises(ValueError, match="código inválido"):
        claude_window.send_code("own-fixture", code)
    assert calls == [], "entrada inválida não pode chegar à janela nem ao processo"


@pytest.mark.parametrize("code", ["synthetic\nsecond-command", "synthetic\rsecond-command", "synthetic\x00tail"])
def test_window_bridge_rejects_control_code_before_account_lookup(monkeypatch, code):
    from types import SimpleNamespace

    from app import account_bridge, config, runtime_coordinator
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(instance="fixture-instance"))
    lookups = []
    monkeypatch.setattr(config, "list_config_dirs", lambda: lookups.append(True) or [])
    body = {"instance": "fixture-instance", "key": {"provider": "claude", "canonical_home": "unused"},
            "operation": "a" * 32, "action": "code", "code": code}
    with pytest.raises(ValueError, match="código inválido"):
        account_bridge.ClaudeWindows().run(body)
    assert lookups == [], "entrada inválida deve ser recusada antes de resolver uma conta"


def test_window_bridge_has_no_cache_to_invalidate(monkeypatch):
    from types import SimpleNamespace

    from app import account_bridge, runtime_coordinator
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(instance="fixture-instance"))
    body = {"instance": "fixture-instance", "key": {"provider": "claude", "canonical_home": "unused"},
            "operation": "a" * 32, "action": "invalidate", "code": None}
    with pytest.raises(ValueError, match="ação inválida"):
        account_bridge.ClaudeWindows().run(body)


def _conta(tmp_path: Path, nome: str) -> Path:
    d = tmp_path / f".claude-{nome}"
    d.mkdir(parents=True)
    return d


def test_pasta_confiada_devolve_a_primeira_existente(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "confia")
    (d / ".claude.json").write_text(json.dumps({"projects": {
        str(tmp_path / "sumiu"): {"hasTrustDialogAccepted": True},
        str(repo): {"hasTrustDialogAccepted": True},
    }}), encoding="utf-8")
    assert claude_window.trusted_folder(d) == repo


def test_pasta_nao_confiada_nao_conta(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "naoconfia")
    (d / ".claude.json").write_text(json.dumps({"projects": {
        str(repo): {"hasTrustDialogAccepted": False},
    }}), encoding="utf-8")
    assert claude_window.trusted_folder(d) is None


def test_claude_json_estragado_nao_levanta(tmp_path):
    d = _conta(tmp_path, "json-ruim")
    (d / ".claude.json").write_text("{ nao é json", encoding="utf-8")
    assert claude_window.trusted_folder(d) is None
