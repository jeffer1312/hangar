from app.pricing import PROVEDORES, slim


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
