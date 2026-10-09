"""Primitivas da janela escondida do login Claude, que o Rust opera por `ClaudeWindows`.

O código colado vai por stdin (`_shell_code`) e nunca chega à janela nem ao processo com caractere
de controle, nem à resolução da conta pela ponte.
"""
import pytest

from app import login_conta


@pytest.mark.parametrize("code", ["synthetic\nsecond-command", "synthetic\rsecond-command", "synthetic\x00tail"])
def test_protected_code_rejects_control_characters_before_io(monkeypatch, code):
    calls = []
    monkeypatch.setattr(login_conta.tmux, "paste_via_clipboard", lambda *args: calls.append("clipboard") or True)
    monkeypatch.setattr(login_conta.tmux, "send_keys", lambda *args: calls.append("keys") or True)
    monkeypatch.setattr(login_conta.tmux, "_run", lambda *args, **kwargs: calls.append("process"))
    with pytest.raises(ValueError, match="código inválido"):
        login_conta._shell_code("own-fixture", code)
    assert calls == [], "entrada inválida não pode chegar à janela nem ao processo"


@pytest.mark.parametrize("code", ["synthetic\nsecond-command", "synthetic\rsecond-command", "synthetic\x00tail"])
def test_window_bridge_rejects_control_code_before_account_lookup(monkeypatch, code):
    from types import SimpleNamespace

    from app import account_bridge, config, runtime_coordinator
    monkeypatch.setattr(runtime_coordinator, "current", lambda: SimpleNamespace(instance="fixture-instance"))
    lookups = []
    monkeypatch.setattr(config, "list_config_dirs", lambda: lookups.append(True) or [])
    body = {"instance": "fixture-instance", "key": {"provider": "claude", "canonical_home": "unused"},
            "operation": "a" * 32, "action": "code", "code": code}
    with pytest.raises(ValueError, match="código inválido"):
        account_bridge.ClaudeWindows().run(body)
    assert lookups == [], "entrada inválida deve ser recusada antes de resolver uma conta"
