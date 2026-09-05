"""Argumentos de modelo e esforço: validar antes, montar depois.

O que esta suíte trava: id que quebraria o shell — ou que viraria uma FLAG — é RECUSADO (o comando
vira string única e é executado como `exec {command}` por `$SHELL -c` em tmux.py:391); cada provider
produz a flag do SEU binário; nível fora da lista fechada não passa; e
provider fora de escopo continua funcionando quando ninguém pediu modelo (senão a criação de sessão
Codex, que é caminho vivo, passaria a devolver 400 sem motivo).
"""
import pytest

from app import model_args as ma


@pytest.mark.parametrize("mau", [
    "k3; touch /tmp/x", "k3 && rm -rf /", "k3 $(whoami)", "k3 `id`",
    "k3 modelo", "k3\nrm -rf /", "k3|tee /tmp/x", "", "x" * 129,
    "k3\n", "k3-256k\n",
    # Valor começando com `-`: higiene, não injeção. Medido — `claude --model --version` consome
    # `--version` como VALOR do --model. O que isso produz é sessão com modelo inválido, e o valor
    # vem de catálogo de provedor; recusar aqui é mais barato que descobrir na sessão.
    "--dangerously-skip-permissions", "-opus", "--model",
])
def test_id_perigoso_e_recusado(mau):
    with pytest.raises(ValueError):
        ma.validar("claude", mau, None)


@pytest.mark.parametrize("bom", [
    "k3", "k3-256k", "cx/gpt-5.6-sol-high", "clinepass/cline-pass/glm-5.2",
    "deepseek-v4-flash", "claude-opus-5", "opus", "gpt-4.1_mini", "a:b",
    # B1 da revisão final: o catálogo real do Pi traz 11 ids com `~` (ex. openrouter). Sem o
    # caractere na whitelist, a tela oferecia a linha e o POST devolvia 400. `~` dentro do
    # argumento citado por shlex.join não sofre expansão do shell.
    "openrouter/~anthropic/claude-opus-latest",
    # B1 da segunda revisão final: é o formato que o próprio Claude Code usa pra marcar a janela de
    # contexto, e está no `settings.json` das duas contas do usuário. Recusar aqui estourava na
    # RETOMADA de uma sessão dessas, quando o modelo é lido do /proc do processo que já rodava.
    "opus[1m]", "claude-opus-5[1m]",
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
    """Criar sessão Kimi com esforço é o caso: estourar sem ninguém ter pedido nada faria TODO
    POST daquele provider devolver 400."""
    assert ma.validar("kimi", None, None) == (None, None)
    assert ma.args_de("kimi", None, None) == []


def test_provider_fora_de_escopo_estoura_se_alguem_pedir_esforco():
    with pytest.raises(ValueError):
        ma.validar("kimi", None, "high")


def test_codex_aceita_modelo_e_esforco():
    assert ma.validar("codex", "gpt-5.6-luna", "xhigh") == ("gpt-5.6-luna", "xhigh")


def test_codex_nao_tem_lista_fechada_de_nivel():
    """Os níveis do Codex são POR MODELO e vêm do provedor (`model/list`): `ultra` existe no
    gpt-5.6-sol e não no gpt-5.5, e `max` não existe no gpt-5.5 — medido em 30/08/2026, codex-cli
    0.151.0. Uma tupla aqui esconderia metade do catálogo do usuário."""
    assert ma.validar("codex", "gpt-5.6-sol", "ultra")[1] == "ultra"


@pytest.mark.parametrize("torto", ["", "a", "x" * 33, "high high", "high;id", "--high", "High\n"])
def test_codex_recusa_nivel_com_forma_torta(torto):
    """Sem lista fechada, o que resta é a forma: o valor vira `-c model_reasoning_effort="..."`
    dentro do comando do pane."""
    with pytest.raises(ValueError):
        ma.validar("codex", "gpt-5.6-sol", torto)


def test_codex_nao_produz_flag_de_outro_binario():
    """A escolha do Codex viaja no lançador (`hangar-codex-tui --model/--effort`), que a traduz pra
    `-m` e pra sobrescrita de config. Deixar `args_de` devolver `--model` aqui seria a flag de OUTRO
    binário entrando calada no comando do pane."""
    with pytest.raises(ValueError):
        ma.args_de("codex", "gpt-5.6-luna", None)
    # Sem escolha nenhuma continua sendo o caminho vivo de sempre.
    assert ma.args_de("codex", None, None) == []


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


def test_modos_permissao_const_existe():
    assert ma.MODOS_PERMISSAO_CLAUDE == ("acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan")


def test_permissao_modo_valido_passa():
    for modo in ma.MODOS_PERMISSAO_CLAUDE:
        ma.validar("claude", None, None, permission_mode=modo)


def test_permissao_modo_none_passa():
    ma.validar("claude", None, None, permission_mode=None)
    ma.validar("claude", None, None)


def test_permissao_modo_invalido_rejeita():
    with pytest.raises(ValueError):
        ma.validar("claude", None, None, permission_mode="invalido")


def test_permissao_args_de_claude_acrescenta_flag():
    assert ma.args_de("claude", None, None, permission_mode="plan") == ["--permission-mode", "plan"]


def test_permissao_args_de_none_nao_acrescenta():
    assert ma.args_de("claude", None, None, permission_mode=None) == []
    assert ma.args_de("claude", None, None) == []
