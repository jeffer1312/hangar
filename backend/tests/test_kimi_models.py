"""Catálogo do config.toml do Kimi + read-back da linha "Switched to …".

O que esta suíte trava: o parse das seções [models."<alias>"] (a chave context_length casa com o
ModelOption do front — renomear derruba a etiqueta de contexto da tela de abertura calado); a
recusa de alias com espaço/controle ANTES de virar tecla na TUI; e a regra da confirmação — a
linha da troca ANTERIOR continua no scrollback, então baseline igual nunca prova nada.
"""
import pytest

from app import kimi_models as km

CONFIG_FALSO = """
default_model = "apikey/k3"

[thinking]
enabled = true
effort = "high"

[models."apikey/k3"]
model = "k3"
display_name = "K3"
max_context_size = 1048576
support_efforts = [ "low", "high", "max" ]
default_effort = "high"

[models."apikey/k3-256k"]
model = "k3-256k"
display_name = "K3-256k"
max_context_size = 262144
support_efforts = [ "low", "high", "max" ]
default_effort = "high"

[models."kimi-code/kimi-for-coding"]
model = "kimi-for-coding"
display_name = "K2.7 Coding"
max_context_size = 262144
"""


@pytest.fixture
def cat(tmp_path):
    p = tmp_path / "config.toml"
    p.write_text(CONFIG_FALSO, encoding="utf-8")
    return km.read_catalog(p)


def test_read_catalog_mapeia_os_campos(cat):
    assert cat["default"] == "apikey/k3"
    assert len(cat["models"]) == 3
    k3 = next(m for m in cat["models"] if m["alias"] == "apikey/k3")
    assert k3["name"] == "K3"
    assert k3["provider"] == "apikey" and k3["id"] == "k3"
    assert k3["context_length"] == 1048576
    assert k3["efforts"] == ["low", "high", "max"]
    # Sem support_efforts no config: lista vazia, nunca None (o front itera).
    coding = next(m for m in cat["models"] if m["alias"] == "kimi-code/kimi-for-coding")
    assert coding["efforts"] == [] and coding["default_effort"] is None


def test_read_catalog_ausente_ou_vazio_e_none(tmp_path):
    assert km.read_catalog(tmp_path / "nao-existe.toml") is None
    p = tmp_path / "config.toml"
    p.write_text('default_model = "apikey/k3"\n', encoding="utf-8")
    assert km.read_catalog(p) is None
    p.write_text("não é toml válido [[[", encoding="utf-8")
    assert km.read_catalog(p) is None


def test_clean_alias_recusa_o_que_viraria_tecla_errada():
    # strip() primeiro: quebra de linha na BORDA é normalizada, não recusada. As ruins são as
    # embutidas (quebrariam o token em duas teclas) e o caractere de controle.
    for ruim in ("", "  ", "apikey/ k3", "k3\n4", "k\ts", "\x1bk3"):
        with pytest.raises(km.KimiModelError) as e:
            km.clean_alias(ruim)
        assert e.value.status == 422, ruim


def test_check_known_devolve_a_entrada_ou_recusa(cat):
    assert km.check_known(cat, "apikey/k3-256k")["name"] == "K3-256k"
    with pytest.raises(km.KimiModelError) as e:
        km.check_known(cat, "apikey/k4")
    assert e.value.status == 422


def test_parse_switched_pega_a_ultima_e_marca_session_only():
    pane = (
        "   Switched to K2.7 Coding with thinking on.\n"
        "\n"
        "   Switched to K3 with thinking high for this session only.\n"
        " 🤖 K3 (high✦) │ 📁 tmp\n"
    )
    sw = km.parse_switched(pane)
    assert sw["name"] == "K3"
    assert sw["session_only"] is True
    assert km.parse_switched("nenhuma troca por aqui\n🤖 K3") is None


def test_confirms_exige_linha_nova_com_o_nome_pedido():
    antes = {"name": "K2.7 Coding", "session_only": False, "raw": "Switched to K2.7 Coding …"}
    # Baseline intacta: a linha velha continua na tela — não prova nada (k3-256k -> k3 medido).
    assert km.confirms(antes, antes, "K3") is False
    # Linha nova com OUTRO nome: a busca casou o item errado — nunca confirmar.
    assert km.confirms({"name": "K2.7 Coding Highspeed", "session_only": True, "raw": "…"},
                       antes, "K3") is False
    # Linha nova com o nome pedido: confirmou.
    assert km.confirms({"name": "K3", "session_only": True, "raw": "…"}, antes, "K3") is True
    assert km.confirms(None, antes, "K3") is False


def test_check_effort_valida_contra_o_suporte_do_modelo(cat):
    k3 = km.check_known(cat, "apikey/k3")
    assert km.check_effort(k3, "LOW") == "low"
    with pytest.raises(km.KimiModelError) as e:
        km.check_effort(k3, "xhigh")
    assert e.value.status == 422
    # Modelo SEM níveis (kimi-for-coding não declara support_efforts): recusa qualquer pedido.
    coding = km.check_known(cat, "kimi-code/kimi-for-coding")
    with pytest.raises(km.KimiModelError):
        km.check_effort(coding, "low")


def test_parse_thinking_set_e_confirms_effort():
    pane = (
        "   Thinking set to low for this session only.\n"
        " 🤖 K3 (low✦)\n"
    )
    sw = km.parse_thinking_set(pane)
    assert sw["level"] == "low"
    assert km.parse_thinking_set("nada por aqui") is None
    antes = {"level": "high", "raw": "Thinking set to high for this session only."}
    assert km.confirms_effort(antes, antes, "low") is False      # linha VELHA não prova nada
    assert km.confirms_effort(sw, antes, "low") is True
    assert km.confirms_effort(sw, antes, "max") is False


def test_parse_thinking_row_le_niveis_e_colchete():
    pane = (
        "  Select a model  (type to search)\n"
        "   ❯ K3                     apikey ← current\n"
        "  Thinking  (←→ to switch)\n"
        "     Low    [ High ]    Max\n"
        " 🤖 K3 (high✦)\n"
    )
    row = km.parse_thinking_row(pane)
    assert row["levels"] == ["Low", "High", "Max"]
    assert row["current"] == "High"
    assert km.parse_thinking_row("sem picker aqui") is None
