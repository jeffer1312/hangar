import pytest

from app import pricing


@pytest.fixture(autouse=True)
def _limpo(tmp_path, monkeypatch):
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path)
    pricing.invalidar_cache()
    yield
    pricing.invalidar_cache()


def test_canoniza_apelidos_de_motor():
    assert pricing.canonizar("k3") == "kimi-k3"
    assert pricing.canonizar("k3-256k") == "kimi-k3"
    assert pricing.canonizar("kimi-for-coding") == "kimi-k3"
    assert pricing.canonizar("cx/gpt-5.6-sol-high") == "gpt-5.6-sol"


def test_canoniza_ids_antigos_pra_tarifa_de_hoje():
    # Ids que o histórico ainda grava mas o models.dev renomeou: sem o alias, o volume deles sairia
    # como "sem tarifa" com traço, e o custo real (recalculado com o preço de hoje, como o app já
    # faz com o histórico inteiro) sumiria.
    assert pricing.canonizar("claude-sonnet-4") == "claude-sonnet-4-5-20250929"
    assert pricing.canonizar("claude-haiku-4.5") == "claude-haiku-4-5-20251001"
    assert pricing.canonizar("deepseek-v4-flash-0731") == "deepseek-v4-flash"
    # Os alvos são DATADOS de propósito — é a forma que as sessões atuais gravam no campo model
    # (medido no histórico real). O alias tem que convergir com o que o log grava hoje, senão o
    # mesmo modelo vira duas linhas no painel. E o invariante que importa de verdade: id antigo
    # agora TEM tarifa (antes rate_for devolvia None e o volume saía como "sem tarifa").
    assert pricing.canonizar("claude-haiku-4.5") == pricing.canonizar("claude-haiku-4-5-20251001")
    assert pricing.rate_for("claude-sonnet-4") is not None
    assert pricing.rate_for("claude-haiku-4.5") is not None
    assert pricing.rate_for("deepseek-v4-flash-0731") is not None


def test_canoniza_prefixo_de_provedor_do_pi():
    assert pricing.canonizar("cline-pass/kimi-k3") == "kimi-k3"
    assert pricing.canonizar("openrouter/deepseek/deepseek-v4-flash") == "deepseek-v4-flash"


def test_id_cru_que_existe_no_catalogo_ganha_do_desmonte_de_prefixo():
    # Há modelo cujo nome CONTÉM barra. Se o desmonte rodasse primeiro, ele sumiria.
    assert pricing.canonizar("claude-opus-5") == "claude-opus-5"


def test_bare_e_prefixado_convergem_na_mesma_linha():
    """canonizar() é a chave de agrupamento do painel "Por modelo": se a forma nua e a
    'provedor/nua' não caem no mesmo id, o mesmo modelo vira duas linhas de gasto."""
    assert pricing.canonizar("claude-sonnet-5") == pricing.canonizar("anthropic/claude-sonnet-5")
    assert pricing.canonizar("kimi-k3") == pricing.canonizar("moonshotai/kimi-k3")
    assert pricing.canonizar("deepseek-v4-flash") == pricing.canonizar("deepseek/deepseek-v4-flash")


def test_rate_do_snapshot_traz_provedor_e_origem():
    r = pricing.rate_for("k3")
    assert r is not None
    assert (r.input, r.output) == (3.0, 15.0)
    assert r.provider == "moonshotai"
    assert r.origin == "snapshot"


def test_modelo_desconhecido_e_none_nunca_zero():
    assert pricing.rate_for("modelo-que-nao-existe-2099") is None


def test_sem_cache_publicado_marca_estimado_e_cobra_como_input():
    r = pricing.rate_for("kimi-k3")   # models.dev não publica cache_write pra ele
    assert r.cache_estimado is True
    assert r.cache_write == r.input


def test_provedor_derivado_do_modelo():
    # É o que resolve a sessão de motor do Claude, onde CP_ENGINE não existe mais.
    assert pricing.provider_for("k3") == "moonshotai"
    assert pricing.provider_for("gpt-5.6-sol") == "openai"


def test_override_ganha_do_snapshot():
    pricing.gravar_override("kimi-k3", {"input": 1.0, "output": 2.0,
                                        "cache_read": 0.1, "cache_write": 1.0})
    r = pricing.rate_for("k3")
    assert (r.input, r.output) == (1.0, 2.0)
    assert r.origin == "override"
    assert r.cache_estimado is False


def test_canoniza_apelido_de_provedor():
    """Cada fonte entrega o próprio vocabulário pro mesmo lugar onde a fatura cai: sem traduzir,
    a assinatura da OpenAI ficava partida entre 'openai' e 'openai-codex'. 'clinepass' NÃO entra
    aqui — é gateway de modelo misturado, como openrouter (ver
    test_canonizar_provedor_nao_encosta_na_conta_nem_no_gateway)."""
    assert pricing.canonizar_provedor("openai-codex") == "openai"
    assert pricing.canonizar_provedor("kimi-coding") == "moonshotai"
    assert pricing.canonizar_provedor("moonshotai") == "moonshotai"


def test_canonizar_provedor_nao_encosta_na_conta_nem_no_gateway():
    # Conta Anthropic é IDENTIDADE, não apelido: achatar juntaria duas contas numa linha só.
    conta = "anthropic:758a9521-e2ef-435b-8738-bc502547c24c"
    assert pricing.canonizar_provedor(conta) == conta
    # Gateway com modelo misturado fica como está: ali a fatura cai mesmo no gateway, e mapear
    # pelo modelo de hoje inventaria a origem de amanhã.
    assert pricing.canonizar_provedor("openrouter") == "openrouter"
    assert pricing.canonizar_provedor("") == ""


def test_custo_aplica_tarifa_por_tipo_de_token():
    r = pricing.rate_for("claude-opus-5")
    c = pricing.custo(r, entrada=1_000_000, saida=1_000_000, cw=1_000_000, cr=1_000_000)
    assert c == {"input": 5.0, "output": 25.0, "cache_write": 6.25, "cache_read": 0.5}


@pytest.mark.parametrize("model,prices", [
    ("gpt-6-astra", (10.0, 1.0, 12.5, 50.0)),
    ("gpt-5.6-sol", (4.0, 0.4, 5.0, 20.0)),
    ("gpt-5.6-luna", (0.2, 0.02, 0.25, 1.2)),
])
def test_tarifas_codex_snapshot_e_contexto_longo(model, prices):
    rate = pricing.rate_for(model)
    assert rate is not None
    assert (rate.input, rate.cache_read, rate.cache_write, rate.output) == prices
    assert pricing.rate_codex(rate, model, False) == rate
    long_rate = pricing.rate_codex(rate, model, True)
    assert (long_rate.input, long_rate.cache_read, long_rate.cache_write, long_rate.output) == (
        prices[0] * 2, prices[1] * 2, prices[2] * 2, prices[3] * 1.5)


def test_contexto_longo_nao_inventa_regra_para_outro_modelo():
    rate = pricing.rate_for("claude-opus-5")
    assert pricing.rate_codex(rate, "claude-opus-5", True) == rate


def test_contexto_longo_preserva_tarifa_manual():
    rate = pricing.Rate(1, 2, 0.1, 1.25, "openai", "override", False)
    assert pricing.rate_codex(rate, "gpt-6-astra", True) == rate


def test_openai_anterior_ao_5_6_nao_cobra_escrita_de_cache():
    # A OpenAI só cobra escrita de cache do GPT-5.6 em diante, e nesses o catálogo publica o
    # preço. Cobrar como input nos anteriores inventava uma linha que a fatura não tem.
    r = pricing.rate_for("gpt-5.5")
    assert r.cache_write == 0.0
    assert r.cache_estimado is False, "zero aqui é preço publicado, não estimativa"
    novo = pricing.rate_for("gpt-6-astra")
    assert novo.cache_write == novo.input * 1.25


def test_escrita_gratis_e_so_da_openai():
    # Em outro provedor, cache sem preço publicado continua cobrado como input e MARCADO.
    r = pricing.rate_for("kimi-k3")
    assert r.cache_write == r.input
    assert r.cache_estimado is True


@pytest.mark.parametrize("crudo", ["apikey/k3", "kimi-code/k3"])
def test_credencial_do_kimi_no_id_nao_apaga_a_tarifa(crudo):
    assert pricing.canonizar(crudo) == "kimi-k3"
    assert pricing.rate_for(crudo) is not None


def test_caixa_do_log_nao_parte_o_modelo_em_duas_linhas():
    # O Pi grava 'DeepSeek-V4-Flash'; o catálogo registra 'deepseek-v4-flash'.
    assert pricing.canonizar("DeepSeek-V4-Flash") == "deepseek-v4-flash"
    assert pricing.rate_for("DeepSeek-V4-Flash") == pricing.rate_for("deepseek-v4-flash")


def test_modo_rapido_dobra_a_tabela_inteira():
    # Opus 5 em fast custa 10/50 no lugar de 5/25, e os multiplicadores de cache incidem sobre
    # esse preço — por isso as quatro colunas dobram juntas.
    r = pricing.rate_for("claude-opus-5")
    f = pricing.rate_fast(r, "claude-opus-5")
    assert (f.input, f.output, f.cache_read, f.cache_write) == (
        r.input * 2, r.output * 2, r.cache_read * 2, r.cache_write * 2)


def test_modo_rapido_nao_toca_modelo_que_nao_oferece_o_modo():
    # Opus 4.6 roda em velocidade padrão e cobra padrão mesmo com o pedido de fast.
    for modelo in ("claude-sonnet-5", "claude-opus-4-6", "gpt-6-astra"):
        r = pricing.rate_for(modelo)
        if r is not None:
            assert pricing.rate_fast(r, modelo) == r
