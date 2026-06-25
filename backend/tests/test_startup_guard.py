import pytest
from app.main import startup_guard


class _FakeSettings:
    def __init__(self, auth_token, lan_bind_ip):
        self.auth_token = auth_token
        self.lan_bind_ip = lan_bind_ip


def test_default_token_non_loopback_raises():
    """Default token + non-loopback IP must abort startup."""
    s = _FakeSettings(auth_token="change-me", lan_bind_ip="192.168.1.50")
    with pytest.raises(SystemExit):
        startup_guard(s)


def test_default_token_loopback_ok(capsys):
    """Default token on loopback is allowed (warns but does not raise)."""
    s = _FakeSettings(auth_token="change-me", lan_bind_ip="127.0.0.1")
    startup_guard(s)  # must not raise
    captured = capsys.readouterr()
    assert "WARNING" in captured.out


def test_real_token_non_loopback_ok():
    """Strong token on any interface must not raise."""
    s = _FakeSettings(auth_token="supersecret", lan_bind_ip="192.168.1.50")
    startup_guard(s)  # must not raise


def test_real_token_loopback_ok():
    """Strong token on loopback is also fine."""
    s = _FakeSettings(auth_token="supersecret", lan_bind_ip="127.0.0.1")
    startup_guard(s)  # must not raise
