"""Normalização do /v1/models: os provedores concordam em `id` e `context_length` e divergem no resto."""
import pytest

from app import engine_probe


def test_normaliza_o_formato_do_kimi(monkeypatch):
    # Medido em api.kimi.com/coding: capability vem como supports_image_in.
    bruto = {"data": [{"id": "k3", "context_length": 262144, "supports_image_in": True}]}
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: bruto)
    assert engine_probe.listar_modelos("https://api.kimi.com/coding", "sk-x") == [
        {"id": "k3", "context_length": 262144, "vision": True}
    ]


def test_normaliza_o_formato_do_omniroute(monkeypatch):
    # Medido em ai.omniwise.com.br: capability vem dentro de "capabilities" e sem flag de imagem.
    bruto = {"data": [{"id": "cx/gpt-5.6-sol", "context_length": 500000,
                       "capabilities": {"tool_calling": True, "thinking": True}}]}
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: bruto)
    assert engine_probe.listar_modelos("https://ai.omniwise.com.br", "sk-x") == [
        {"id": "cx/gpt-5.6-sol", "context_length": 500000, "vision": None}
    ]


def test_modelo_sem_id_e_ignorado(monkeypatch):
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: {"data": [{"context_length": 1}]})
    assert engine_probe.listar_modelos("https://x.y", "sk-x") == []


def test_resposta_sem_data_estoura_com_mensagem(monkeypatch):
    monkeypatch.setattr(engine_probe, "_buscar", lambda b, k: {"nada": 1})
    with pytest.raises(RuntimeError, match="inesperada"):
        engine_probe.listar_modelos("https://x.y", "sk-x")
