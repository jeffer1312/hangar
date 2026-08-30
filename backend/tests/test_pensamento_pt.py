"""Traducao do resumo do pensamento: os tetos e o que acontece quando o provedor falha."""
import pytest

from app import pensamento_pt
from app.narrar import NarrarError


@pytest.fixture(autouse=True)
def _limpa_cache():
    pensamento_pt._cache.clear()
    yield
    pensamento_pt._cache.clear()


def test_falha_do_provedor_devolve_o_original(monkeypatch):
    # O bloco ja esta na tela quando esta chamada sai: trocar o conteudo por erro apagaria o que a
    # pessoa esta lendo. Silenciar e a decisao CERTA aqui, e por isso ela tem teste.
    def explode(*a, **k):
        raise NarrarError(503, "sem chave")
    monkeypatch.setattr(pensamento_pt, "chamar_chat", explode)
    assert pensamento_pt.traduzir("thinking about it") == "thinking about it"


def test_texto_gigante_nao_vai_pro_provedor(monkeypatch):
    # No Pi e no Kimi este campo carrega raciocinio CRU, sem tamanho previsivel.
    chamou = []
    monkeypatch.setattr(pensamento_pt, "chamar_chat",
                        lambda *a, **k: chamou.append(1) or "traduzido")
    gigante = "x" * (pensamento_pt.MAX_CHARS + 1)
    assert pensamento_pt.traduzir(gigante) == gigante
    assert chamou == []


def test_prazo_total_devolve_o_resto_como_veio(monkeypatch):
    # As chamadas sao sequenciais e o navegador desiste em 30s: passado o prazo, o que sobra volta
    # como veio em vez de gastar minutos do provedor produzindo texto que ninguem recebe.
    relogio = iter([0.0, 0.0, 100.0, 100.0, 100.0])
    monkeypatch.setattr(pensamento_pt.time, "monotonic", lambda: next(relogio))
    monkeypatch.setattr(pensamento_pt, "chamar_chat", lambda *a, **k: "traduzido")
    assert pensamento_pt.traduzir_varios(["um", "dois", "tres"]) == ["traduzido", "dois", "tres"]


def test_cache_evita_a_segunda_chamada(monkeypatch):
    chamadas = []
    monkeypatch.setattr(pensamento_pt, "chamar_chat",
                        lambda *a, **k: chamadas.append(1) or "traduzido")
    assert pensamento_pt.traduzir("hello") == "traduzido"
    assert pensamento_pt.traduzir("hello") == "traduzido"
    assert len(chamadas) == 1
