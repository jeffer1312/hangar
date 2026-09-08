"""Ciclo periódico com eventos controlados e backend real em processo isolado."""
import asyncio
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import threading
import urllib.request
from urllib.error import HTTPError

import pytest

from app.omp_plugin_sync import PluginSyncLoop
from tests.test_omp_plugin_sync import scenario


def report(action="unchanged", errors=None):
    return {"complete_inventory": True, "items": [{"action": action}], "errors": errors or []}


class FakeSync:
    def __init__(self):
        self.calls = []
        self.failure = False
        self.entered = threading.Event()
        self.release = threading.Event()
        self.block = False
        self.active = 0
        self.max_active = 0

    def import_marketplaces(self, **kwargs):
        self.calls.append("marketplaces")
        return report()

    def reconcile(self, *, stop_requested):
        self.calls.append("plugins")
        self.active += 1
        self.max_active = max(self.max_active, self.active)
        self.entered.set()
        try:
            if self.block:
                assert self.release.wait(5), "Prova não liberou o worker"
            if self.failure:
                raise RuntimeError("Falha sintética")
            return report("install")
        finally:
            self.active -= 1


def observe_cycles(service):
    completed = asyncio.Queue()
    original_wait = service._wait
    async def observed_wait():
        completed.put_nowait(service.status())
        await original_wait()
    service._wait = observed_wait
    return completed


@pytest.mark.asyncio
async def test_desligado_nao_inicia_operacoes():
    sync = FakeSync()
    service = PluginSyncLoop(sync, enabled=False, interval=300)
    await service.start()
    await service.close()
    assert sync.calls == []
    assert service.status()["state"] == "disabled"


@pytest.mark.asyncio
async def test_primeira_passagem_e_nova_passagem_apos_intervalo():
    sync = FakeSync()
    service = PluginSyncLoop(sync, enabled=True, interval=0.02)
    completed = observe_cycles(service)
    await service.start()
    try:
        first = await asyncio.wait_for(completed.get(), 2)
        second = await asyncio.wait_for(completed.get(), 2)
        assert first["state"] == second["state"] == "updated"
        assert second["completed_at"] > first["completed_at"]
        assert sync.calls[:4] == ["marketplaces", "plugins", "marketplaces", "plugins"]
        assert sync.max_active == 1
    finally:
        await service.close()


@pytest.mark.asyncio
async def test_erro_nao_encerra_ciclo_e_proxima_passagem_recupera():
    sync = FakeSync()
    sync.failure = True
    service = PluginSyncLoop(sync, enabled=True, interval=300)
    completed = observe_cycles(service)
    await service.start()
    try:
        failed = await asyncio.wait_for(completed.get(), 2)
        assert failed["state"] == "error"
        sync.failure = False
        service._wake.set()
        recovered = await asyncio.wait_for(completed.get(), 2)
        assert recovered["state"] == "updated"
    finally:
        await service.close()


@pytest.mark.asyncio
async def test_parada_aguarda_worker_sem_marcar_sucesso_nem_repetir():
    sync = FakeSync()
    sync.block = True
    service = PluginSyncLoop(sync, enabled=True, interval=0.001)
    await service.start()
    assert await asyncio.to_thread(sync.entered.wait, 2)
    closing = asyncio.create_task(service.close())
    await asyncio.sleep(0)
    assert not closing.done()
    sync.release.set()
    await asyncio.wait_for(closing, 2)
    assert sync.calls == ["marketplaces", "plugins"]
    assert sync.max_active == 1
    assert service.status()["state"] == "stopped"


@pytest.mark.asyncio
async def test_kill_switch_impede_operacoes_e_retomada_e_controlada():
    permitted = [False]
    sync = FakeSync()
    service = PluginSyncLoop(sync, enabled=True, interval=300, permitted=lambda: permitted[0])
    completed = observe_cycles(service)
    await service.start()
    try:
        paused = await asyncio.wait_for(completed.get(), 2)
        assert paused["state"] == "paused"
        assert sync.calls == []
        permitted[0] = True
        service._wake.set()
        resumed = await asyncio.wait_for(completed.get(), 2)
        assert resumed["state"] == "updated"
    finally:
        await service.close()


def _serve_test_backend():
    """Entry point do processo de prova; efeitos alheios ao worker ficam inertes."""
    import uvicorn
    from app import api
    async def idle(*args, **kwargs):
        await asyncio.Event().wait()
    for name in ("_fetch_loop", "_auto_update_loop", "_prune_loop", "_renova_token_loop"):
        setattr(api, name, idle)
    api.hook_state.watch = idle
    api.stall_watch.watch = idle
    api.registry.list = lambda: []
    api.pricing.atualizar_em_background = lambda: None
    api._usd_brl = lambda: None
    api.automations_enabled = lambda: True
    class SlowSync:
        def __init__(self, **kwargs):
            pass
        def import_marketplaces(self, **kwargs):
            return report()
        def reconcile(self, **kwargs):
            os.write(int(os.environ["TEST_ENTERED_FD"]), b"1")
            os.read(int(os.environ["TEST_RELEASE_FD"]), 1)
            return report()
    api.PluginSynchronizer = SlowSync
    class ObservedLoop(PluginSyncLoop):
        async def close(self):
            await super().close()
            Path(os.environ["TEST_STOPPED_FILE"]).write_text(json.dumps(self.status()), encoding="utf-8")
    api.PluginSyncLoop = ObservedLoop
    sock = socket.socket(fileno=int(os.environ["TEST_HTTP_FD"]))
    server = uvicorn.Server(uvicorn.Config(api.app, log_level="error", lifespan="on"))
    server.run(sockets=[sock])


@pytest.mark.skipif(os.environ.get("HANGAR_TEST_SANDBOX") != "1", reason="Backend real exige sandbox")
def test_backend_real_responde_enquanto_worker_aguarda(tmp_path):
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    entered_read, entered_write = os.pipe()
    release_read, release_write = os.pipe()
    environment = dict(os.environ, CP_OMP_PLUGIN_SYNC_ENABLED="1", CP_OMP_PLUGIN_SYNC_INTERVAL="300",
                       CP_AUTH_TOKEN="fixture-sync-token", TEST_HTTP_FD=str(listener.fileno()),
                       TEST_ENTERED_FD=str(entered_write), TEST_RELEASE_FD=str(release_read),
                       TEST_STOPPED_FILE=str(tmp_path / "stopped.json"),
                       HOME=str(tmp_path), USERPROFILE=str(tmp_path), CLAUDE_CONFIG_DIR=str(tmp_path / ".claude"))
    process = subprocess.Popen(
        [sys.executable, "-c", "from tests.test_omp_plugin_sync_loop import _serve_test_backend; _serve_test_backend()"],
        cwd=Path(__file__).resolve().parents[1], env=environment,
        pass_fds=(listener.fileno(), entered_write, release_read), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    try:
        ready, _, _ = select.select([entered_read], [], [], 10)
        assert ready, "Worker não iniciou no backend real"
        assert os.read(entered_read, 1) == b"1"
        request = urllib.request.Request(
            f"http://127.0.0.1:{listener.getsockname()[1]}/api/omp/plugin-sync",
            headers={"Authorization": "Bearer fixture-sync-token"},
        )
        with pytest.raises(HTTPError) as unauthorized:
            urllib.request.urlopen(request.full_url, timeout=3)
        assert unauthorized.value.code == 401
        with urllib.request.urlopen(request, timeout=3) as response:
            status = json.load(response)
        assert status["state"] == "running"
        assert process.poll() is None
    finally:
        os.write(release_write, b"1")
        process.terminate()
        try:
            out, err = process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            out, err = process.communicate()
        listener.close()
        for fd in (entered_read, entered_write, release_read, release_write):
            os.close(fd)
    assert process.returncode in (0, -15), err.decode(errors="replace")
    assert json.loads((tmp_path / "stopped.json").read_text())["state"] == "stopped"


def test_parada_durante_precondicao_nao_inicia_mutacao(scenario):
    stopped = threading.Event()
    listings = 0
    def stop_during_precondition(payload):
        nonlocal listings
        listings += 1
        if listings == 2:
            stopped.set()
        return payload
    scenario.native.list_transform = stop_during_precondition
    scenario.reconcile(stop_requested=stopped.is_set)
    assert scenario.native.mutations == []
    scenario.native.list_transform = None
    stopped.clear()
    assert scenario.reconcile()["errors"] == []
    assert scenario.name in scenario.native.package["dependencies"]


@pytest.mark.asyncio
async def test_parada_observada_nao_e_esquecida_se_kill_switch_voltar():
    permitted = [True]
    sync = FakeSync()
    original = sync.import_marketplaces
    toggled = False
    def interrupt_once(**kwargs):
        nonlocal toggled
        result = original(**kwargs)
        if not toggled:
            toggled = True
            permitted[0] = False
            assert kwargs["stop_requested"]()
            permitted[0] = True
        return result
    sync.import_marketplaces = interrupt_once
    service = PluginSyncLoop(sync, enabled=True, interval=300, permitted=lambda: permitted[0])
    completed = observe_cycles(service)
    await service.start()
    try:
        interrupted = await asyncio.wait_for(completed.get(), 2)
        assert interrupted["state"] == "paused"
        assert sync.calls == ["marketplaces"]
        service._wake.set()
        recovered = await asyncio.wait_for(completed.get(), 2)
        assert recovered["state"] == "updated"
        assert sync.calls == ["marketplaces", "marketplaces", "plugins"]
    finally:
        await service.close()


@pytest.mark.parametrize("interval", [0, float("inf")])
def test_config_recusa_intervalo_que_impede_ciclo_limitado(interval):
    from app.config import Settings
    from pydantic import ValidationError
    with pytest.raises(ValidationError):
        Settings(_env_file=None, omp_plugin_sync_interval=interval)


def test_intervalo_vazio_no_ambiente_nao_impede_subida(monkeypatch):
    from app.config import Settings
    monkeypatch.setenv("CP_OMP_PLUGIN_SYNC_INTERVAL", "")
    assert Settings(_env_file=None).omp_plugin_sync_interval == 300


@pytest.mark.asyncio
async def test_suspensao_tem_prioridade_sobre_outro_item_atualizado():
    sync = FakeSync()
    sync.import_marketplaces = lambda **kwargs: report("import")
    sync.reconcile = lambda **kwargs: report("suspended")
    service = PluginSyncLoop(sync, enabled=True, interval=300)
    completed = observe_cycles(service)
    await service.start()
    try:
        status = await asyncio.wait_for(completed.get(), 2)
        assert status["state"] == "suspended"
        assert status["last_report"]["marketplaces"]["items"][0]["action"] == "import"
    finally:
        await service.close()


def test_mudanca_de_layout_nao_migra_ledger_existente(scenario):
    from app.omp_plugin_sync import PluginSynchronizer
    sync = PluginSynchronizer(home=scenario.home, claude_dir=scenario.claude_dir, runner=scenario.native)
    assert sync.reconcile()["errors"] == []
    ledger = scenario.home / ".hangar/omp-plugin-sync.json"
    before = ledger.read_bytes()
    (Path(sync.env["XDG_DATA_HOME"]) / "omp").mkdir(parents=True)
    scenario.native.calls.clear()
    result = sync.reconcile()
    assert result["complete_inventory"] is False and result["errors"]
    assert scenario.native.calls == []
    assert ledger.read_bytes() == before
    assert scenario.name in scenario.native.package["dependencies"]
