from pathlib import Path
from app.config import _default_projects_dir, detect_lan_ip, pairing_url_api, resolve_bind_ip, pairing_url, Settings
from app.config import (
    _default_projects_dir,
    detect_lan_ip,
    pairing_url,
    porta_do_front,
    resolve_bind_ip,
    Settings,
)


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
def test_porta_do_front_cai_no_backend_quando_nao_ha_servico_de_front():
    # Sem serviço de front instalado (o padrão desde que o backend passou a servir o dist), o QR
    # e o painel de alcance têm de apontar pra porta do BACKEND. Com 5173 cravado como default,
    # os dois mandavam a pessoa pra uma porta onde ninguém escuta.
    # front_port=0 explícito: hermético contra um backend/.env local com CP_FRONT_PORT=5173
    # (quem mantém o preview tem isso gravado, e o teste passava só no CI, que não tem .env).
    assert porta_do_front(Settings(port=8765, front_port=0)) == 8765
    assert porta_do_front(Settings(port=9000, front_port=0)) == 9000
    assert porta_do_front(Settings(port=8765, front_port=5173)) == 5173


def test_front_port_vazio_nao_derruba_o_backend():
    # `CP_FRONT_PORT=` no .env levantava ValidationError, e como `settings = Settings()` roda no
    # import do módulo, o backend inteiro não subia — sem tela e sem mensagem que explicasse.
    assert porta_do_front(Settings(front_port="", port=8765)) == 8765  # type: ignore[arg-type]
    assert porta_do_front(Settings(front_port="  ", port=8765)) == 8765  # type: ignore[arg-type]


def test_pairing_url_usa_a_porta_do_backend_sem_front_port():
    s = Settings(lan_bind_ip="192.168.1.50", auth_token="tok", public_url="", port=8765, front_port=0)
    assert pairing_url(s) == "http://192.168.1.50:8765/?token=tok"
