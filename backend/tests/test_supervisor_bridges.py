"""Supervisor liga as pontes só depois da saúde; todos os pedidos usam transporte sintético."""
import asyncio

import pytest


def bridge():
    from app import list_bridge
    return list_bridge


def test_supervisor_enables_only_after_health_and_clears_even_without_proc(monkeypatch):
    from app import rust_server
    t = bridge()
    calls = []
    original = t.configure
    def configure(address, secret):
        calls.append((address, secret))
        original(address, secret)
    monkeypatch.setattr(t, "configure", configure)
    class Process:
        stdin = None
        stdout = None
        def poll(self):
            return None
    monkeypatch.setattr(rust_server, "_spawn", lambda *args: Process())
    # A partida do runtime (2B) não é o assunto aqui: resposta válida e sem transporte real.
    monkeypatch.setattr(rust_server, "_runtime_ready", lambda proc, instance: {
        "type": "runtime_ready", "protocol": rust_server.RUST_SERVER_PROTOCOL, "instance": instance, "port": 12348})
    monkeypatch.setattr(rust_server.Supervisor, "configure_runtime", lambda *args: None)
    def health(*args):
        assert t._config is None
        return {"ok": True, "protocol": rust_server.RUST_SERVER_PROTOCOL, "terminal_panel": True, "owns": [],
                "terminal_address": "127.0.0.1:12347"}
    monkeypatch.setattr(rust_server, "_health", health)
    monkeypatch.setattr(rust_server, "server_log_path", lambda: "/tmp/unused-test-log")
    supervisor = rust_server.Supervisor(None, "0.0.0.0", 12345, 12346, "owner", "", lambda: False)
    async def run():
        assert await supervisor._start() == "up"
        assert t._config[0] == "127.0.0.1:12347"
        supervisor.proc = None
        await supervisor.stop()
        assert t._config is None
    asyncio.run(run())
    assert calls[0] == (None, None)
    assert calls[-1] == (None, None)


@pytest.mark.parametrize("address", [None, "198.51.100.1:8765", "localhost:8765", "127.0.0.1:0", "0.0.0.0:8765", "http://127.0.0.1:8765"])
def test_supervisor_bad_health_address_is_startup_failure(monkeypatch, caplog, address):
    from app import rust_server
    t = bridge()
    class Process:
        stdin = None
        stdout = None
        def poll(self): return None
    monkeypatch.setattr(rust_server, "_spawn", lambda *args: Process())
    # A partida do runtime (2B) não é o assunto aqui: resposta válida e sem transporte real.
    monkeypatch.setattr(rust_server, "_runtime_ready", lambda proc, instance: {
        "type": "runtime_ready", "protocol": rust_server.RUST_SERVER_PROTOCOL, "instance": instance, "port": 12348})
    monkeypatch.setattr(rust_server.Supervisor, "configure_runtime", lambda *args: None)
    monkeypatch.setattr(rust_server, "server_log_path", lambda: "/tmp/unused-test-log")
    monkeypatch.setattr(rust_server, "_health", lambda *args: dict(ok=True, protocol=rust_server.RUST_SERVER_PROTOCOL,
                                                                    terminal_panel=True, terminal_address=address))
    supervisor = rust_server.Supervisor(None, "0.0.0.0", 12345, 12346, "owner", "", lambda: False)
    # Sem endereço privado válido o Rust não sobe pela metade: o Python assume a porta inteira.
    assert asyncio.run(supervisor._start()) == "address"
    # `terminal_panel` também devolve "address": o log diz que foi o endereço que barrou.
    assert "sem endereço privado válido" in caplog.text
    assert t._config is None
