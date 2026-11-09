import asyncio
import os
import socket
from pathlib import Path
import pytest

pytestmark = pytest.mark.skipif(os.name == "nt", reason="Fixture de Git usa shell POSIX")
_BINARY = Path(__file__).parents[2] / "crates/target/debug/hangar-server"


def test_supervisor_starts_runtime_and_workspace_bridges_together(tmp_path, monkeypatch):
    from app import (account_bridge, groups_bridge, internal_api, runtime_coordinator, runtime_queue,
                     rust_server)

    monkeypatch.setattr(runtime_coordinator, "_current", None)
    monkeypatch.setattr(runtime_queue, "_coordinator", None)
    monkeypatch.setattr(rust_server, "server_log_path", lambda: tmp_path / "server.log")
    # O binário real varre canos órfãos ao subir: nunca com o HOME e os processos desta máquina.
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("CP_RUST_NO_ORPHAN_SWEEP", "1")
    monkeypatch.setenv("CP_RUST_SERVER_BIN", str(_BINARY))
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    supervisor = rust_server.Supervisor(_BINARY, "127.0.0.1", port, 9,
                                        "fixture-owner", "127.0.0.1", lambda: False)

    async def scenario():
        try:
            assert await supervisor._start() == "up"
            assert supervisor.runtime_transport is not None
            configured = account_bridge.private_transport() is not None
            assert configured, "a ponte de contas não recebeu o endereço privado"
            configured = groups_bridge._config is not None
            assert configured, "a ponte de grupos não recebeu o endereço privado"
        finally:
            await supervisor.stop()
            internal_api.set_secret(None)
        assert supervisor.runtime_transport is None
        # O segredo interno não pode seguir para uma porta que o filho morto liberou.
        cleared = account_bridge.private_transport() is None
        assert cleared, "a ponte de contas ainda guarda o endereço do filho encerrado"
        cleared = groups_bridge._config is None
        assert cleared, "a ponte de grupos ainda guarda o endereço do filho encerrado"

    asyncio.run(scenario())
