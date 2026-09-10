"""Traducao do resumo do pensamento: os tetos e o que acontece quando o provedor falha."""
import pytest

from app import pensamento_pt
from app.narrar import NarrarError


@pytest.fixture(autouse=True)
def _limpa_cache():
    pensamento_pt._cache.clear()
    pensamento_pt._pausado_ate = 0.0
    yield
    pensamento_pt._cache.clear()
    pensamento_pt._pausado_ate = 0.0


def test_falha_pausa_o_provedor_e_nao_usa_o_plano_b(monkeypatch):
    # Com o provedor em 500, cada bloco visivel disparava chamadas de 12s + um `claude -p` por
    # falha, em paralelo, e o backend inteiro engasgava (lista estourando 4s, chat abrindo em 8s).
    chamadas = []
    def explode(*a, **k):
        chamadas.append(k.get("plano_b"))
        raise NarrarError(502, "provedor 500")
    monkeypatch.setattr(pensamento_pt, "chamar_chat", explode)
    assert pensamento_pt.traduzir("one") == "one"
    assert pensamento_pt.traduzir("two") == "two"     # em pausa: nem chama o provedor
    assert chamadas == [False]
    pensamento_pt._pausado_ate = 0.0
    assert pensamento_pt.traduzir("three") == "three"
    assert chamadas == [False, False]


def test_sem_vaga_devolve_o_original_mas_o_cache_ainda_responde(monkeypatch):
    chamadas = []
    monkeypatch.setattr(pensamento_pt, "chamar_chat", lambda *a, **k: chamadas.append(1) or "pt")
    assert pensamento_pt.traduzir("ja") == "pt"        # entra no cache
    # As duas vagas ocupadas: quem precisa do provedor nao espera na fila, devolve como veio;
    # o que ja esta no cache responde igual (a vaga so vale pra chamada ao provedor).
    assert pensamento_pt._vagas.acquire(blocking=False)
    assert pensamento_pt._vagas.acquire(blocking=False)
    try:
        assert pensamento_pt.traduzir_varios(["a", "ja"]) == ["a", "pt"]
        assert chamadas == [1]
    finally:
        pensamento_pt._vagas.release()
        pensamento_pt._vagas.release()
    assert pensamento_pt.traduzir_varios(["a"]) == ["pt"]


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
