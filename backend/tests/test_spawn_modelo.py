"""Do corpo do POST até o comando que sobe no pane."""
import pytest

from app.adapters import get_adapter


def test_claude_spawn_sem_escolha_e_o_de_hoje():
    assert get_adapter("claude").spawn_command("/tmp", "sid") == ["claude", "--session-id", "sid"]


def test_claude_spawn_com_escolha():
    assert get_adapter("claude").spawn_command("/tmp", "sid", model="opus", effort="max") == [
        "claude", "--session-id", "sid", "--model", "opus", "--effort", "max"]


def test_pi_spawn_com_escolha_usa_thinking():
    assert get_adapter("pi").spawn_command("/tmp", "sid", model="kimi-coding/k3", effort="high") == [
        "pi", "--session-id", "sid", "--model", "kimi-coding/k3", "--thinking", "high"]


def test_codex_spawn_continua_recusando_com_a_assinatura_nova():
    """Sem os kwargs novos isso daria TypeError de argumento inesperado; com eles, a recusa de
    hoje é preservada. O teste prova as duas coisas de uma vez."""
    with pytest.raises(NotImplementedError):
        get_adapter("codex").spawn_command("/tmp", "sid", model=None, effort=None)


def test_id_hostil_nao_chega_no_comando():
    with pytest.raises(ValueError):
        get_adapter("claude").spawn_command("/tmp", "sid", model="k3; touch /tmp/x", effort=None)


def test_claude_spawn_com_permissao_plan():
    assert get_adapter("claude").spawn_command("/tmp", "sid", None, None, permission_mode="plan") == [
        "claude", "--session-id", "sid", "--permission-mode", "plan"]


def test_claude_spawn_sem_permissao_identico_ao_de_hoje():
    assert get_adapter("claude").spawn_command("/tmp", "sid", None, None, permission_mode=None) == [
        "claude", "--session-id", "sid"]
    assert get_adapter("claude").spawn_command("/tmp", "sid", None, None) == [
        "claude", "--session-id", "sid"]


def test_pi_spawn_ignora_permissao():
    base = get_adapter("pi").spawn_command("/tmp", "sid", None, None)
    assert get_adapter("pi").spawn_command("/tmp", "sid", None, None, permission_mode="plan") == base


def test_kimi_spawn_ignora_permissao():
    base = get_adapter("kimi").spawn_command("/tmp", "sid", None, None)
    assert get_adapter("kimi").spawn_command("/tmp", "sid", None, None, permission_mode="plan") == base
