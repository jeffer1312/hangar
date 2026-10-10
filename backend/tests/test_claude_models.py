"""O adaptador preserva conta, formato e causa da falha sem executar o CLI em Python."""
from pathlib import Path
import pytest
from app import claude_models, dictation_bridge


def test_catalog_uses_the_requested_account_and_returns_raw_models(monkeypatch):
    raw = [{"value": "model-real", "displayName": "Modelo"}]
    def bridge(operation, payload):
        assert operation == "catalog_models"
        assert payload["provider"] == "claude"
        assert payload["home"] == str(Path("conta-selecionada"))
        return {"models": [{"id": "model-real"}], "raw": raw}
    monkeypatch.setattr(dictation_bridge, "request", bridge)
    assert claude_models.listar(Path("conta-selecionada")) == raw


def test_rust_failure_keeps_the_missing_cli_distinct(monkeypatch):
    def missing(*args):
        raise dictation_bridge.BridgeError(502, "dictation_cli_missing", "CLI indisponível")
    monkeypatch.setattr(dictation_bridge, "request", missing)
    with pytest.raises(claude_models.ClaudeAusente, match="CLI indisponível"):
        claude_models.listar()


def test_catalog_cannot_be_replaced_with_aliases_after_rust_failure(monkeypatch):
    def unavailable(*args):
        raise dictation_bridge.BridgeError(503, "dictation_rust_unavailable", "Rust indisponível")
    monkeypatch.setattr(dictation_bridge, "request", unavailable)
    with pytest.raises(claude_models.ClaudeIndisponivel, match="Rust indisponível"):
        claude_models.listar()
