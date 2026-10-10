"""Transporte do catálogo Codex: identidade e erros permanecem ligados ao Rust."""
from pathlib import Path
import pytest
from app import codex_models, dictation_bridge


def test_catalog_uses_the_requested_account_without_a_python_cache(monkeypatch):
    models = [{"id": "model-real", "efforts": ["low"], "service_tiers": []}]
    def bridge(operation, payload):
        assert operation == "catalog_models"
        assert payload == {"provider": "codex", "home": str(Path("conta-selecionada")), "fresh": True}
        return {"models": models, "raw": []}
    monkeypatch.setattr(dictation_bridge, "request", bridge)
    assert codex_models.listar(True, codex_home=Path("conta-selecionada")) == models


@pytest.mark.parametrize("code,kind", [
    ("dictation_cli_missing", codex_models.CodexAusente),
    ("dictation_catalog_rate_limited", codex_models.CodexLimitado),
    ("dictation_rust_unavailable", codex_models.CodexIndisponivel),
])
def test_catalog_failure_never_falls_back_to_another_executor(monkeypatch, code, kind):
    def unavailable(*args):
        raise dictation_bridge.BridgeError(503, code, "Catálogo indisponível")
    monkeypatch.setattr(dictation_bridge, "request", unavailable)
    with pytest.raises(kind, match="Catálogo indisponível"):
        codex_models.listar()


def test_validation_error_from_rust_is_a_value_error_for_existing_consumers(monkeypatch):
    def bridge(operation, payload):
        if operation == "catalog_models":
            return {"models": [{"id": "model-real", "efforts": ["low"]}]}
        assert operation == "validate_model"
        assert payload["effort"] == "high"
        raise dictation_bridge.BridgeError(400, "dictation_model_unavailable", "Nível fora do suporte")
    monkeypatch.setattr(dictation_bridge, "request", bridge)
    with pytest.raises(ValueError, match="Nível fora do suporte"):
        codex_models.checar_escolha("model-real", "high")


def test_raw_capacity_query_preserves_account_configuration_and_cli_version(monkeypatch):
    model = {"slug": "model-real", "context_window": 200000}
    def bridge(operation, payload):
        assert operation == "raw_model"
        assert payload == {"home": "conta-selecionada", "config": {"model_provider": "openai"},
                           "model": "model-real", "version": "0.162.1"}
        return {"model": model}
    monkeypatch.setattr(dictation_bridge, "request", bridge)
    assert codex_models.raw_model(Path("conta-selecionada"), {"model_provider": "openai"}, "model-real", "0.162.1") == model
