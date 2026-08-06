import json
from pathlib import Path

import pytest

from app import pricing
from app.pricing import PROVEDORES, slim

FIXTURE = Path(__file__).parent / "fixtures" / "models_dev_recorte.json"


@pytest.fixture(autouse=True)
def _limpo(tmp_path, monkeypatch):
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path)
    pricing.invalidar_cache()
    yield
    pricing.invalidar_cache()


def test_slim_so_aceita_provedor_de_primeira_mao():
    bruto = {
        "moonshotai": {"models": {"kimi-k3": {"cost": {"input": 3, "output": 15, "cache_read": 0.3}}}},
        # A armadilha real: uma revenda publica um modelo chamado 'k3' de graça. Se ele entrar no
        # catálogo, toda sessão de Kimi vira US$ 0,00 sem avisar ninguém.
        "revenda-qualquer": {"models": {"k3": {"cost": {"input": 0, "output": 0}}}},
    }
    out = slim(bruto)
    assert "kimi-k3" in out
    assert out["kimi-k3"]["provider"] == "moonshotai"
    assert "k3" not in out, "modelo de provedor fora da lista branca não pode entrar"


def test_slim_descarta_preco_zero_do_proprio_provedor_canonico():
    bruto = {"openai": {"models": {"gpt-gratis": {"cost": {"input": 0, "output": 0}}}}}
    assert slim(bruto) == {}


def test_slim_mantem_cache_ausente_como_none():
    bruto = {"moonshotai": {"models": {"kimi-k3": {"cost": {"input": 3, "output": 15, "cache_read": 0.3}}}}}
    assert slim(bruto)["kimi-k3"]["cache_write"] is None


def test_lista_de_provedores_e_fechada():
    # Este teste FALHA de propósito se alguém ampliar a lista. Ampliar é permitido — mas tem que
    # ser uma decisão consciente, com o teste atualizado junto, porque o custo de errar é
    # tarifa zero silenciosa (ver test_slim_so_aceita_provedor_de_primeira_mao).
    assert PROVEDORES == (
        "anthropic", "openai", "moonshotai", "zhipuai",
        "deepseek", "google", "xai", "mistral",
    )


def test_canoniza_apelidos_de_motor():
    assert pricing.canonizar("k3") == "kimi-k3"
    assert pricing.canonizar("k3-256k") == "kimi-k3"
    assert pricing.canonizar("kimi-for-coding") == "kimi-k3"
    assert pricing.canonizar("cx/gpt-5.6-sol-high") == "gpt-5.6-sol"


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


def test_parse_do_formato_real_do_models_dev():
    bruto = json.loads(FIXTURE.read_text())
    cat = pricing.catalogo_de_bruto(bruto)
    assert cat["kimi-k3"].provider == "moonshotai"
    assert "k3" not in cat, "modelo de revenda com preço zero não pode entrar"


def test_slim_descarta_entrada_sem_preco_de_input():
    """A Task 2 é a primeira a passar dado AO VIVO pro slim(); a guarda da Task 1 só descartava
    quando input E output faltavam, então `{"output": 5}` sem input estourava KeyError. Sem
    preço de input não há régua pra nada aqui (cache e equivalente são relativos a ele), então
    a entrada não serve — descartar é a resposta certa, não remendar."""
    bruto = {"openai": {"models": {"meio-preco": {"cost": {"output": 5}}}}}
    assert pricing.slim(bruto) == {}


def test_slim_descarta_entrada_sem_preco_de_output():
    """Caso ESPELHO do de cima, e é o que mata a atualização de tarifa: um modelo de embedding
    (input, sem output) publicado por provedor canônico fazia float(c["output"]) levantar
    KeyError, o _baixar() engolia com um warning genérico e o catálogo congelava no snapshot
    pra sempre. Hoje são 0 entradas assim — a guarda é pro dia em que houver uma."""
    bruto = {"openai": {"models": {"text-embedding-9": {"cost": {"input": 0.02}}}}}
    assert pricing.slim(bruto) == {}
    # E não pode derrubar o catálogo INTEIRO junto com a entrada ruim.
    misto = {"openai": {"models": {
        "text-embedding-9": {"cost": {"input": 0.02}},
        "gpt-bom": {"cost": {"input": 1, "output": 2}},
    }}}
    assert list(pricing.slim(misto)) == ["gpt-bom", "openai/gpt-bom"]


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


def test_download_manda_user_agent_proprio(monkeypatch):
    """O models.dev responde 403 ao UA default do urllib (medido na Task 1). Sem header, a
    atualização de tarifa falha calada e o app fica preso no snapshot pra sempre."""
    visto = {}

    def falso_urlopen(req, timeout=None):
        visto["ua"] = req.get_header("User-agent")
        raise OSError("corta aqui — só queremos inspecionar o Request")

    monkeypatch.setattr(pricing.urllib.request, "urlopen", falso_urlopen)
    monkeypatch.setattr(pricing, "_ultima_tentativa", 0.0)
    pricing._baixar()
    assert visto.get("ua"), "Request tem que carregar User-Agent próprio"
    assert "urllib" not in visto["ua"].lower()


def test_download_preserva_cache_read_quando_so_cache_write_falta(monkeypatch):
    """O deepseek publica cache_read mas não cache_write. `cache_estimado` (true com QUALQUER um
    ausente) fazia o payload gravado zerar os DOIS, e o cache_read real (0.0028) sumia: na
    releitura, cache lido era cobrado a preço de input (medido: deepseek 50x a mais, kimi-k3 85%
    do custo inflado). O cache gravado tem que preservar cada campo, não os dois juntos."""
    bruto = {"deepseek": {"models": {
        "deepseek-v4-flash": {"cost": {"input": 0.14, "output": 0.28,
                                       "cache_read": 0.0028, "cache_write": None}},
    }}}

    class Resp:
        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def read(self):
            return json.dumps(bruto).encode()

    monkeypatch.setattr(pricing.urllib.request, "urlopen", lambda req, timeout=None: Resp())
    monkeypatch.setattr(pricing, "_ultima_tentativa", 0.0)
    pricing._baixar()

    m = json.loads((pricing._CACHE_DIR / "models.dev.json").read_text())["modelos"]["deepseek-v4-flash"]
    assert m["cache_read"] == 0.0028
    assert m["cache_write"] is None
    # E a releitura cobra cache lido pelo preço dele, não pelo input:
    r = pricing.rate_for("deepseek-v4-flash")
    assert r is not None and r.cache_read == 0.0028 and r.cache_write == 0.14


def test_custo_aplica_tarifa_por_tipo_de_token():
    r = pricing.rate_for("claude-opus-5")
    c = pricing.custo(r, entrada=1_000_000, saida=1_000_000, cw=1_000_000, cr=1_000_000)
    assert c == {"input": 5.0, "output": 25.0, "cache_write": 6.25, "cache_read": 0.5}
