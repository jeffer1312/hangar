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


def test_codex_spawn_devolve_o_lancador_e_ignora_escolha_de_modelo():
    """O Codex nasce como os outros: um comando no pane, o lançador.

    Modelo/esforço são aceitos na assinatura (é o Protocol) e NÃO entram no comando: escolher
    modelo na criação de sessão Codex é recusado na API, e obedecer aqui faria a escolha sumir
    calada num lugar e valer noutro."""
    assert get_adapter("codex").spawn_command("/tmp", "sid", None, None, None) == [
        "hangar-codex-tui", "--cwd", "/tmp"]
    assert get_adapter("codex").spawn_command(
        "/tmp", "sid", "gpt-5.6-luna", "high", "plan") == ["hangar-codex-tui", "--cwd", "/tmp"]
    assert get_adapter("codex").spawn_command(
        "/tmp", "sid", initial_prompt="revise") == [
            "hangar-codex-tui", "--cwd", "/tmp", "--prompt", "revise"]


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
