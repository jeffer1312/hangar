
import pytest

from app import costs_cache as cc, costs_sources as cs, pricing


@pytest.fixture(autouse=True)
def _limpo(tmp_path, monkeypatch):
    monkeypatch.setattr(cc, "_CACHE_DIR", tmp_path / "cache")
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path / "pricing")
    cs.invalidar_cache()
    yield
    cs.invalidar_cache()


def _isolar(monkeypatch, varrer, ler=lambda desde=None: []) -> None:
    """Estado do aquecimento/última varredura zerado, varredura e leitura falsas — a real
    varreria o `~/.claude` da máquina de dentro da thread e o teste esperaria minutos."""
    import threading
    monkeypatch.setattr(cs, "_aquecido", threading.Event())
    monkeypatch.setattr(cs, "_aquecedor", None)
    monkeypatch.setattr(cs, "_refrescador", None)
    monkeypatch.setattr(cs, "_ultima_coleta", None)
    monkeypatch.setattr(cs, "_sincronizar", varrer)
    monkeypatch.setattr(cs, "_ler_custos", ler)


def test_pedido_antes_da_primeira_coleta_dispara_aquecimento_e_responde_aquecendo(monkeypatch):
    """O pedido nunca paga a varredura fria: dispara a thread e devolve `Aquecendo`; depois
    que ela termina, o pedido lê o índice e responde."""
    import threading

    segurar = threading.Event()
    chamadas = []

    def varrer_lento():
        chamadas.append(1)
        segurar.wait(5)

    _isolar(monkeypatch, varrer_lento)
    with pytest.raises(cs.Aquecendo):
        cs.coletar_ou_aquecendo()
    assert cs._aquecedor is not None and cs._aquecedor.is_alive()
    with pytest.raises(cs.Aquecendo):
        cs.coletar_ou_aquecendo()   # segunda chamada não abre outra thread
    assert chamadas == [1]
    segurar.set()
    cs._aquecedor.join(5)
    assert cs._aquecido.is_set()
    assert cs.coletar_ou_aquecendo(esperar=1.0) == []
    assert chamadas == [1]                          # aquecida: lê sem varrer
    assert cs.coletar_ou_aquecendo(esperar=1.0, fresco=True) == []
    assert chamadas == [1, 1]


def test_depois_de_aquecido_o_pedido_le_na_hora_e_atualiza_atras(monkeypatch):
    """Tela abre na hora com o índice como está; passado o frescor, uma varredura nova roda em
    thread. `fresco` (Atualizar dados) varre no pedido."""
    import threading

    n = {"v": 0}
    segurar = threading.Event()

    def varrer_contado():
        n["v"] += 1
        if n["v"] == 2:
            segurar.wait(5)   # a varredura de fundo demora: o pedido não pode esperar por ela

    _isolar(monkeypatch, varrer_contado, lambda desde=None: ["lido"])
    with pytest.raises(cs.Aquecendo):
        cs.coletar_ou_aquecendo()
    cs._aquecedor.join(5)
    assert cs.coletar_ou_aquecendo() == ["lido"]     # fresca: lê sem varrer
    assert cs.coletar_ou_aquecendo() == ["lido"]
    assert n["v"] == 1
    monkeypatch.setattr(cs, "_FRESCOR_S", 0.0)       # envelheceu
    assert cs.coletar_ou_aquecendo() == ["lido"]     # responde na hora, sem esperar a varredura…
    assert cs._refrescador is not None and cs._refrescador.is_alive()
    segurar.set()
    cs._refrescador.join(5)
    assert n["v"] == 2                               # …que rodou atrás
    assert cs.coletar_ou_aquecendo(fresco=True) == ["lido"]
    assert n["v"] == 3


def test_aquecimento_que_falha_nao_prende_a_tela_em_aquecendo(monkeypatch):
    def explode():
        raise OSError("disco")

    _isolar(monkeypatch, explode)
    with pytest.raises(cs.Aquecendo):
        cs.coletar_ou_aquecendo()
    cs._aquecedor.join(5)
    assert cs._aquecido.is_set()
    with pytest.raises(OSError):
        cs.coletar_ou_aquecendo()   # o erro aparece no pedido, não some
