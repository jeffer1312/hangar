from pathlib import Path
from app.config import _default_projects_dir, detect_lan_ip, pairing_url_api, resolve_bind_ip, pairing_url, Settings


def test_default_projects_dir_honors_claude_config_dir(monkeypatch):
    """The transcript dir must follow $CLAUDE_CONFIG_DIR, not a hardcoded ~/.claude —
    machines/users set CLAUDE_CONFIG_DIR to different locations."""
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/tmp/some-custom-config")
    assert _default_projects_dir() == Path("/tmp/some-custom-config/projects")


def test_default_projects_dir_falls_back_to_home(monkeypatch):
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    assert _default_projects_dir() == Path.home() / ".claude" / "projects"


def test_detect_lan_ip_returns_ipv4():
    ip = detect_lan_ip()
    assert isinstance(ip, str) and ip.count(".") == 3


def test_resolve_bind_ip_passthrough_when_not_auto():
    assert resolve_bind_ip(Settings(lan_bind_ip="192.168.1.50")) == "192.168.1.50"


def test_pairing_url_uses_public_url_when_set():
    s = Settings(public_url="https://pocket.local/", auth_token="tok")
    assert pairing_url(s) == "https://pocket.local/?token=tok"


def test_pairing_url_builds_from_bind_ip_and_front_port():
    # The QR points at the PWA front (front_port), not the API port.
    # public_url="" explicito: hermetico contra um backend/.env local que defina CP_PUBLIC_URL
    # (senao o fallback por bind-ip nao seria exercitado).
    s = Settings(lan_bind_ip="192.168.1.50", front_port=5173, auth_token="tok", public_url="")
    assert pairing_url(s) == "http://192.168.1.50:5173/?token=tok"


def test_pairing_url_api_usa_porta_do_backend():
    # public_url="" de propósito: backend/.env desta máquina tem CP_PUBLIC_URL e o Settings herda
    s = Settings(lan_bind_ip="10.0.0.5", port=8765, auth_token="t", public_url="")
    assert pairing_url_api(s) == "http://10.0.0.5:8765/?token=t"


def test_pairing_url_api_com_public_url_leva_api_na_query():
    s = Settings(lan_bind_ip="10.0.0.5", port=8765, auth_token="t", public_url="https://casa.ts.net/")
    assert pairing_url_api(s) == "https://casa.ts.net/?token=t&api=http://10.0.0.5:8765"
