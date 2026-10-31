"""Do corpo do POST até o comando que sobe no pane."""
import pytest

from app.adapters import get_adapter


@pytest.fixture(autouse=True)
def _isolate_plugins(monkeypatch):
    from app import plugin_bridge
    monkeypatch.setattr(plugin_bridge, "raizes_dos_plugins", lambda: ["/fixture/plugin"])


def test_claude_spawn_sem_escolha_e_o_de_hoje():
    assert get_adapter("claude").spawn_command("/tmp", "sid") == ["claude", "--session-id", "sid", "--plugin-dir", "/fixture/plugin"]


def test_claude_spawn_com_escolha():
    assert get_adapter("claude").spawn_command("/tmp", "sid", model="opus", effort="max") == [
        "claude", "--session-id", "sid", "--plugin-dir", "/fixture/plugin", "--model", "opus", "--effort", "max"]


def test_pi_spawn_com_escolha_usa_thinking():
    assert get_adapter("pi").spawn_command("/tmp", "sid", model="kimi-coding/k3", effort="high") == [
        "pi", "--session-id", "sid", "--model", "kimi-coding/k3", "--thinking", "high"]


def test_id_hostil_nao_chega_no_comando():
    with pytest.raises(ValueError):
        get_adapter("claude").spawn_command("/tmp", "sid", model="k3; touch /tmp/x", effort=None)


def test_claude_spawn_com_permissao_plan():
    assert get_adapter("claude").spawn_command("/tmp", "sid", None, None, permission_mode="plan") == [
        "claude", "--session-id", "sid", "--plugin-dir", "/fixture/plugin", "--permission-mode", "plan"]


def test_claude_spawn_sem_permissao_identico_ao_de_hoje():
    assert get_adapter("claude").spawn_command("/tmp", "sid", None, None, permission_mode=None) == [
        "claude", "--session-id", "sid", "--plugin-dir", "/fixture/plugin"]
    assert get_adapter("claude").spawn_command("/tmp", "sid", None, None) == [
        "claude", "--session-id", "sid", "--plugin-dir", "/fixture/plugin"]


def test_pi_spawn_ignora_permissao():
    base = get_adapter("pi").spawn_command("/tmp", "sid", None, None)
    assert get_adapter("pi").spawn_command("/tmp", "sid", None, None, permission_mode="plan") == base
