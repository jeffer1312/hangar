"""Argumentos de modelo e esforço: validar antes, montar depois.

O que esta suíte trava: id que quebraria o shell é RECUSADO (o comando é montado por concatenação em
registry.py:1114 e executado como `exec {command}` por `$SHELL -c` em tmux.py:391 — sem quoting no
caminho); cada provider produz a flag do SEU binário; nível fora da lista fechada não passa; e
provider fora de escopo continua funcionando quando ninguém pediu modelo (senão a criação de sessão
Codex, que é caminho vivo, passaria a devolver 400 sem motivo).
"""
import pytest

from app import model_args as ma


@pytest.mark.parametrize("mau", [
    "k3; touch /tmp/x", "k3 && rm -rf /", "k3 $(whoami)", "k3 `id`",
    "k3 modelo", "k3\nrm -rf /", "k3|tee /tmp/x", "", "x" * 129,
])
def test_id_perigoso_e_recusado(mau):
    with pytest.raises(ValueError):
        ma.validar("claude", mau, None)


@pytest.mark.parametrize("bom", [
    "k3", "k3-256k", "cx/gpt-5.6-sol-high", "clinepass/cline-pass/glm-5.2",
    "deepseek-v4-flash", "claude-opus-5", "opus", "gpt-4.1_mini", "a:b",
])
def test_id_legitimo_passa(bom):
    assert ma.validar("claude", bom, None)[0] == bom


def test_claude_produz_model_e_effort():
    assert ma.args_de("claude", "opus", "max") == ["--model", "opus", "--effort", "max"]


def test_pi_produz_model_e_thinking():
    """Flag diferente de propósito: o binário do Pi não conhece --effort, e passar a errada mata o
    processo no arranque com o pane já criado — o app reportaria sessão que não existe."""
    assert ma.args_de("pi", "kimi-coding/k3", "high") == [
        "--model", "kimi-coding/k3", "--thinking", "high"]


def test_um_provider_nao_vaza_a_flag_do_outro():
    assert "--thinking" not in ma.args_de("claude", "opus", "max")
    assert "--effort" not in ma.args_de("pi", "kimi-coding/k3", "high")


def test_sem_escolha_nao_produz_argumento_nenhum():
    assert ma.args_de("claude", None, None) == []


def test_provider_fora_de_escopo_passa_quando_ninguem_pediu_modelo():
    """Criar sessão Codex é caminho vivo (api.py:912). Estourar aqui faria TODO POST com
    provider='codex' devolver 400 sem ninguém ter pedido modelo."""
    assert ma.validar("codex", None, None) == (None, None)
    assert ma.args_de("codex", None, None) == []


def test_provider_fora_de_escopo_estoura_se_alguem_pedir_modelo():
    with pytest.raises(ValueError):
        ma.validar("codex", "gpt-5", None)


def test_so_o_esforco_tambem_vale():
    assert ma.args_de("claude", None, "high") == ["--effort", "high"]


def test_nivel_fora_da_lista_do_claude_e_recusado():
    with pytest.raises(ValueError):
        ma.validar("claude", "opus", "off")     # 'off' é do Pi


def test_ultracode_nao_e_valor_de_effort():
    """model_picker.EFFORT_ORDER lista 'ultracode', mas ele é do picker interativo — o binário manda
    'Run /effort ultracode in an interactive terminal'. Como flag de arranque, não existe."""
    with pytest.raises(ValueError):
        ma.validar("claude", "opus", "ultracode")


def test_nivel_do_pi_aceita_off():
    assert ma.validar("pi", "kimi-coding/k3", "off")[1] == "off"
