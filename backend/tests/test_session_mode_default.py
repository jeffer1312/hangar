"""O modo omitido herda; escolha explícita nunca altera a preferência."""
from unittest.mock import AsyncMock, Mock
import pytest
from app import api, runtime_config
from app.registry import SessionInfo


def _sem_pino_do_conftest(monkeypatch):
    # O conftest força headless_default=False para os outros testes; aqui o padrão é o assunto.
    original = runtime_config.get

    def get(campo):
        if campo != "headless_default":
            return original(campo)
        return runtime_config._carregar().get(campo, getattr(runtime_config.settings, campo, None))
    monkeypatch.setattr(runtime_config, "get", get)


@pytest.mark.asyncio
@pytest.mark.parametrize("provider", ["claude", "codex", "pi"])
@pytest.mark.parametrize("default", [False, True])
@pytest.mark.parametrize("requested", [None, False, True])
async def test_default_and_explicit_mode(tmp_path, monkeypatch, provider, default, requested):
    monkeypatch.setattr(runtime_config, "_backend_config_base", lambda: tmp_path)
    monkeypatch.setattr(api.settings, "headless_default", default)
    _sem_pino_do_conftest(monkeypatch)
    create = Mock(return_value=SessionInfo(name="test-mode", cwd=str(tmp_path), provider=provider))
    monkeypatch.setattr(api.registry, "create", create)
    monkeypatch.setattr(api, "get_adapter", Mock())
    monkeypatch.setattr(api, "_aquecer_codex_sem_terminal", AsyncMock())
    body = api.CreateBody(name="test-mode", cwd=str(tmp_path), provider=provider, headless=requested)
    await api._criar_sessao(body, {})
    effective = requested if requested is not None else default and provider in ("claude", "codex")
    assert create.call_args.kwargs.get("headless", False) is effective
    assert runtime_config.get("headless_default") is default
    assert not (tmp_path / "runtime-config.json").exists()
    assert body.headless is requested


@pytest.mark.asyncio
@pytest.mark.parametrize("provider", ["claude", "codex"])
@pytest.mark.parametrize("requested", [None, False, True])
async def test_read_only_requires_terminal_without_overriding_explicit_mode(tmp_path, monkeypatch, provider, requested):
    from app import orq_readonly

    monkeypatch.setattr(runtime_config, "_backend_config_base", lambda: tmp_path)
    monkeypatch.setattr(api.settings, "headless_default", True)
    _sem_pino_do_conftest(monkeypatch)
    prepare = Mock()
    monkeypatch.setattr(orq_readonly, "prepare", prepare)
    # Uma regressão na recusa nunca pode abrir processo real durante este teste.
    for method in ("_create_headless", "_create_codex_headless"):
        monkeypatch.setattr(api.registry, method, Mock(side_effect=AssertionError("unexpected session creation")))
    create = Mock(wraps=api.registry.create) if requested is True else Mock(
        return_value=SessionInfo(name="read-only-mode", cwd=str(tmp_path), provider=provider))
    monkeypatch.setattr(api.registry, "create", create)
    body = api.CreateBody(name="read-only-mode", cwd=str(tmp_path), provider=provider,
                          read_only=True, headless=requested,
                          permission_mode="bypassPermissions" if provider == "claude" else None)
    if requested is True:
        with pytest.raises(api.HTTPException) as rejected:
            await api._criar_sessao(body, {})
        assert rejected.value.status_code == 409
        assert "read_only" in str(rejected.value.detail)
    else:
        await api._criar_sessao(body, {})
    assert create.call_args.kwargs.get("headless", False) is (requested is True)
    assert create.call_args.kwargs["read_only"] is True
    prepare.assert_called()
    assert runtime_config.get("headless_default") is True


@pytest.mark.parametrize("flags, expected", [((), None), (("--headless",), True), (("--terminal",), False)])
def test_cli_preserves_omission_and_explicit_mode(tmp_path, flags, expected):
    import json
    import shutil
    import subprocess
    import threading
    from http.server import BaseHTTPRequestHandler, HTTPServer
    from pathlib import Path

    requests = []
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"claude": {"disponivel": true, "default": true}}')
        def do_POST(self):
            requests.append((self.path, json.loads(self.rfile.read(int(self.headers["Content-Length"])))))
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{}')
        def log_message(self, *args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    scripts = tmp_path / "scripts"
    scripts.mkdir()
    (tmp_path / "backend").mkdir()
    (tmp_path / "backend" / ".env").write_text(f"CP_AUTH_TOKEN=test\nCP_PORT={server.server_port}\n")
    script = scripts / "hangar-send"
    shutil.copy(Path(__file__).parents[2] / "scripts" / "hangar-send", script)
    try:
        result = subprocess.run(["bash", str(script), "--new", "test-mode", "/tmp", *flags], capture_output=True, text=True, timeout=10)
        assert result.returncode == 0, result.stderr
        assert len(requests) == 1 and requests[0][0] == "/api/sessions"
        body = requests[0][1]
        if expected is None:
            assert "headless" not in body
        else:
            assert body["headless"] is expected
        for conflict in (("--headless", "--terminal"), ("--terminal", "--headless")):
            result = subprocess.run(["bash", str(script), "--new", "test-mode", "/tmp", *conflict], capture_output=True, text=True, timeout=10)
            assert result.returncode == 2
        assert len(requests) == 1
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


@pytest.mark.asyncio
async def test_busy_account_is_a_coded_conflict(tmp_path, monkeypatch):
    from app import account_lifecycle

    monkeypatch.setattr(runtime_config, "_backend_config_base", lambda: tmp_path)
    create = Mock(return_value=SessionInfo(name="busy-account", cwd=str(tmp_path), provider="claude"))
    monkeypatch.setattr(api.registry, "create", create)
    monkeypatch.setattr(api, "get_adapter", Mock())

    def busy(*args, **kwargs):
        raise account_lifecycle.AccountLockError("account_busy")

    monkeypatch.setattr(account_lifecycle, "acquire", busy)
    body = api.CreateBody(name="busy-account", cwd=str(tmp_path), provider="claude")
    with pytest.raises(api.HTTPException) as refused:
        await api._criar_sessao(body, {})
    assert refused.value.status_code == 409
    assert refused.value.detail["code"] == "account_busy"
    create.assert_not_called()
