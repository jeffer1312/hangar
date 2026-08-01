import time
from datetime import datetime

import pytest

from app import costs
from app import costs_sources
from app import pricing
from app.costs_sources import UsageRow


@pytest.fixture(autouse=True)
def _isolado(tmp_path, monkeypatch):
    """Tarifa vem do SNAPSHOT do repo, e nada vai à rede.

    Sem o primeiro, um cache de tarifa baixado na máquina (ou um override) muda o custo e o
    teste passa ou falha conforme o disco de quem roda. Sem o segundo, `montar()` chama
    `usd_brl()` de verdade e a suíte paga o timeout da API de câmbio.
    """
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path / "pricing")
    pricing.invalidar_cache()
    monkeypatch.setattr(costs, "_rate", None)
    monkeypatch.setattr(costs, "_rate_at", time.monotonic())
    yield
    pricing.invalidar_cache()


def _linha(**kw):
    base = dict(ts=datetime(2026, 8, 1, 10, tzinfo=costs.LOCAL), source="claude",
                provider="anthropic:u1", model="claude-opus-5", project="/repo/a",
                session_id="s1", input=1_000_000, output=1_000_000,
                cache_write=1_000_000, cache_read=1_000_000, subagente=False)
    return UsageRow(**{**base, **kw})


def test_totais_quebram_o_custo_por_tipo_de_token():
    r = costs.montar([_linha()], now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    k = {b.kind: b for b in r.by_kind}
    assert k["input"].cost == 5.0
    assert k["output"].cost == 25.0
    assert k["cache_write"].cost == 6.25
    assert k["cache_read"].cost == 0.5
    assert r.totals.cost == 36.75
    # Preço cheio: os 3M de input+cw+cr a US$5 mais 1M de output a US$25. Errar aqui é fácil
    # (esquecer o cache_write, ou pesar o cache pela tarifa DE cache em vez da de input).
    assert r.custo_sem_cache == 40.0


def test_modelo_sem_tarifa_nao_soma_e_entra_na_lista_de_sem_tarifa():
    r = costs.montar([_linha(model="modelo-fantasma-2099")],
                     now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert r.totals.cost == 0.0
    assert r.totals.input == 1_000_000, "os TOKENS continuam contando; só o custo é desconhecido"
    assert "modelo-fantasma-2099" in r.sem_tarifa


def test_provedor_atravessa_a_fonte():
    # O caso que motivou o eixo: a mesma assinatura da OpenAI gasta pelo Codex CLI e pelo Pi.
    linhas = [_linha(source="codex", provider="openai", model="gpt-5.6-sol"),
              _linha(source="pi", provider="openai", model="gpt-5.6-sol")]
    r = costs.montar(linhas, now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    prov = {b.key: b for b in r.by_provider}
    assert len(prov) == 1 and prov["openai"].sessions == 2
    assert {b.key for b in r.by_source} == {"codex", "pi"}


def test_provedor_de_conta_leva_rotulo_legivel(monkeypatch):
    """A chave continua sendo o uuid (é ela que soma entre servidores da malha), mas o que a tela
    LÊ é o e-mail: a linha de topo do 'Por provedor', com 87% do gasto, aparecia como
    'anthropic:758a9521-e2ef-435b-8738-bc502547c24c'."""
    monkeypatch.setitem(costs_sources._ROTULOS, "anthropic:u1", "eu@exemplo.com")
    r = costs.montar([_linha(), _linha(provider="openai", model="gpt-5.6-sol")],
                     now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    prov = {b.key: b for b in r.by_provider}
    assert prov["anthropic:u1"].label == "eu@exemplo.com"
    # Provedor cujo nome já é legível não ganha rótulo: o front cai pra própria chave.
    assert prov["openai"].label is None


def test_period_corta_pela_data_do_servidor():
    velha = _linha(ts=datetime(2026, 7, 1, 10, tzinfo=costs.LOCAL), session_id="antiga")
    nova = _linha(ts=datetime(2026, 8, 1, 10, tzinfo=costs.LOCAL))
    r = costs.montar([velha, nova], period="7d",
                     now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert r.totals.sessions == 1
    assert r.applied.period == "7d"


def test_equivalente_cobrado_pesa_cada_tipo_pela_propria_tarifa():
    # Opus 5: in 5, out 25, cw 6.25, cr 0.5 -> pesos 1, 5, 1.25, 0.1 sobre a régua do input.
    # 1M de cada = 1M + 5M + 1.25M + 0.1M = 7.35M equivalentes.
    agora = datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL)
    r = costs.montar([_linha()], now=agora)
    assert r.equivalente_cobrado == 7_350_000

    # E o cache lido puxa pra BAIXO: 10M de cache read pesam como 1M de input (tarifa 0,5 contra
    # 5,0). Com 1M de cada tipo o total sobe, porque o output sozinho pesa 5x — por isso a régua
    # de "cache é barato" se mede numa carga de cache, não numa mistura.
    so_cache = costs.montar([_linha(input=0, output=0, cache_write=0, cache_read=10_000_000)],
                            now=agora)
    assert so_cache.equivalente_cobrado == 1_000_000
    assert so_cache.equivalente_cobrado < so_cache.totals.cache_read


def test_janela_anterior_sai_da_lista_completa():
    now = datetime(2026, 8, 20, 12, tzinfo=costs.LOCAL)
    atual = [_linha(ts=datetime(2026, 8, 20 - d, 10, tzinfo=costs.LOCAL), session_id=f"a{d}")
             for d in range(7)]
    antes = [_linha(ts=datetime(2026, 8, 13 - d, 10, tzinfo=costs.LOCAL), session_id=f"b{d}")
             for d in range(7)]
    r = costs.montar(atual + antes, period="7d", now=now)
    assert r.totals.sessions == 7
    assert r.anterior is not None and r.anterior.sessions == 7


def test_janela_anterior_mal_coberta_nao_vira_comparacao():
    # Foi o ▲574% do mockup: o histórico começava em julho, então "30 dias anteriores" tinha
    # 3 dias de dado. Isso não é crescimento, é o vazio dividindo.
    now = datetime(2026, 8, 20, 12, tzinfo=costs.LOCAL)
    atual = [_linha(ts=datetime(2026, 8, 20, 10, tzinfo=costs.LOCAL))]
    # 10/07 e não 25/07: com period=30d a janela ATUAL começa em 22/07, então 25/07 cairia
    # dentro dela, a janela anterior (22/06–21/07) ficaria vazia, e o None viria do `if not
    # janela` — a regra de cobertura, que é o que este teste existe pra travar, nunca rodaria.
    um_dia_so = [_linha(ts=datetime(2026, 7, 10, 10, tzinfo=costs.LOCAL), session_id="b")]
    r = costs.montar(atual + um_dia_so, period="30d", now=now)
    assert r.anterior is None   # cobertos=1, e 1*3 < 30

    # O contrário, senão um _janela_anterior que só sabe dizer None passaria no teste acima:
    # janela anterior BEM coberta (15 dias com registro, dentro de 22/06–21/07) devolve o bucket.
    bem = [_linha(ts=datetime(2026, 7, 5 + d, 10, tzinfo=costs.LOCAL), session_id=f"c{d}")
           for d in range(15)]
    r2 = costs.montar(atual + bem, period="30d", now=now)
    assert r2.anterior is not None and r2.anterior.sessions == 15


def test_period_all_nao_tem_janela_anterior():
    r = costs.montar([_linha()], period="all",
                     now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert r.anterior is None


def test_sem_tarifa_usa_o_id_canonico():
    # Duas grafias do MESMO modelo não podem virar duas linhas de "sem tarifa".
    linhas = [_linha(model="fantasma-2099", session_id="x"),
              _linha(model="openrouter/fantasma-2099", session_id="y")]
    r = costs.montar(linhas, now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert r.sem_tarifa == ["fantasma-2099"]


def test_fatiar_por_dimensao_nao_cria_nem_some_token():
    """Invariante que veio do test_by_model_tokens_batem_com_o_total, agora em cinco dimensões:
    a soma de cada corte tem que bater com o total. Errar a chave de agrupamento (reusar o
    acumulador entre chaves, por exemplo) dá número plausível e errado — o modo de falha desta
    tela. Tokens primos e diferentes por linha: número redondo mascara erro de soma."""
    now = datetime(2026, 8, 2, 12, tzinfo=costs.LOCAL)
    linhas = [
        _linha(model="claude-opus-5", source="claude", provider="anthropic:u1",
               project="/repo/a", session_id="1", input=7, output=11,
               cache_write=13, cache_read=17,
               ts=datetime(2026, 8, 1, 10, tzinfo=costs.LOCAL)),
        _linha(model="gpt-5.6-sol", source="codex", provider="openai",
               project="/repo/b", session_id="2", input=19, output=23,
               cache_write=29, cache_read=31,
               ts=datetime(2026, 8, 2, 10, tzinfo=costs.LOCAL)),
        # Sem tarifa de propósito: linha que não soma custo não pode sumir da contagem de tokens.
        _linha(model="fantasma-2099", source="pi", provider="kimi-coding",
               project="/repo/c", session_id="3", input=37, output=41,
               cache_write=43, cache_read=47,
               ts=datetime(2026, 8, 2, 11, tzinfo=costs.LOCAL)),
    ]
    r = costs.montar(linhas, now=now)
    for nome in ("by_provider", "by_source", "by_project", "by_model", "by_day"):
        cortes = getattr(r, nome)
        for campo in ("sessions", "input", "output", "cache_write", "cache_read"):
            assert sum(getattr(b, campo) for b in cortes) == getattr(r.totals, campo), \
                f"{nome}.{campo} não bate com o total"
        assert sum(b.cost for b in cortes) == pytest.approx(r.totals.cost), f"{nome}.cost"
    # E o mesmo pelo outro eixo: o custo por tipo de token também é uma fatia do mesmo bolo.
    assert sum(b.cost for b in r.by_kind) == pytest.approx(r.totals.cost)


def test_modelo_ignorado_nao_vira_linha_de_sem_tarifa():
    # '<synthetic>' e 'unknown' não são modelo. rate_for devolve None pra eles igual a um modelo
    # sem preço, mas listá-los sugere preço faltando. Claude já filtra na origem; Codex e Pi não.
    r = costs.montar([_linha(model="<synthetic>"), _linha(model="unknown", session_id="b")],
                     now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert r.sem_tarifa == []
    assert r.totals.sessions == 2, "a linha continua contando token; ela só não vira preço"


def test_rates_dizem_de_onde_veio_o_preco():
    r = costs.montar([_linha()], now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    tarifa = {t.model: t for t in r.rates}["claude-opus-5"]
    assert tarifa.input == 5.0 and tarifa.output == 25.0
    assert tarifa.origin in ("snapshot", "models.dev", "override")
    assert tarifa.cache_estimado is False


def test_usd_brl_cacheia_e_nao_rebate_na_rede(monkeypatch):
    """1ª chamada busca; 2ª usa cache; falha mantém última cotação conhecida."""
    import io

    calls = {"n": 0}

    class FakeResp(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

    def fake_urlopen(url, timeout=None):
        calls["n"] += 1
        return FakeResp(b'{"USDBRL": {"bid": "5.4321"}}')

    monkeypatch.setattr(costs.urllib.request, "urlopen", fake_urlopen)
    monkeypatch.setattr(costs, "_rate", None)
    monkeypatch.setattr(costs, "_rate_at", 0.0)

    assert costs.usd_brl() == 5.4321
    assert costs.usd_brl() == 5.4321  # cache: sem novo hit
    assert calls["n"] == 1

    # Cache expirado + rede fora -> mantém a última cotação, e a falha "conta" como tentativa
    monkeypatch.setattr(costs, "_rate_at", 0.0)

    def boom(url, timeout=None):
        calls["n"] += 1
        raise OSError("offline")

    monkeypatch.setattr(costs.urllib.request, "urlopen", boom)
    assert costs.usd_brl() == 5.4321
    n_after_fail = calls["n"]
    assert costs.usd_brl() == 5.4321  # falha cacheada: não tenta de novo já
    assert calls["n"] == n_after_fail


def test_combos_somam_igual_ao_total():
    """A rota é a mesma e o dado é o mesmo: somar os combos tem que dar o total. Divergência
    aqui é a definição de 'a tela mostra número que o dado não sustenta'. Fixture, não disco:
    o teste que lia a máquina passava VAZIO em CI e falhava sozinho com sessão viva."""
    linhas = [
        _linha(provider="p1", source="claude", project="/a", model="claude-opus-5"),
        _linha(provider="p2", source="pi", project="/b", model="claude-opus-5", session_id="s2"),
        _linha(provider="p1", source="claude", project="/a", model="claude-opus-5", session_id="s3"),
    ]
    r = costs.montar(linhas, now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert abs(sum(c.cost for c in r.combos) - r.totals.cost) < 1e-9
    for campo in ("input", "output", "cache_write", "cache_read", "sessions"):
        assert sum(getattr(c, campo) for c in r.combos) == getattr(r.totals, campo)


def test_combos_agrupam_a_combinacao_repetida():
    """Duas linhas na mesma combinação viram UM combo com sessions=2 — senão o detalhamento
    é só a lista crua e o payload cresce sem motivo."""
    linhas = [
        _linha(provider="p", source="claude", project="/a", model="claude-opus-5"),
        _linha(provider="p", source="claude", project="/a", model="claude-opus-5", session_id="s2"),
    ]
    r = costs.montar(linhas, now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert len(r.combos) == 1
    assert r.combos[0].sessions == 2


def test_combo_separa_subagente():
    """13,7% do volume. Sem a dimensão, o usuário não consegue ver quanto os Task custam."""
    linhas = [
        _linha(provider="p", source="claude", project="/a", model="claude-opus-5"),
        _linha(provider="p", source="claude", project="/a", model="claude-opus-5",
               session_id="s2", subagente=True),
    ]
    r = costs.montar(linhas, now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert len(r.combos) == 2
    assert {c.subagente for c in r.combos} == {True, False}


def test_combos_nao_quebram_o_que_ja_existia():
    """O campo é ACRÉSCIMO: os quatro agrupamentos e os escalares continuam iguais."""
    r = costs.montar([_linha()], now=datetime(2026, 8, 1, 12, tzinfo=costs.LOCAL))
    assert r.by_provider and r.by_source and r.by_project and r.by_model and r.by_day
    assert r.equivalente_cobrado > 0
    assert r.custo_sem_cache > 0
