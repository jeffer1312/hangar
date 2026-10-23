# backend/tests/test_internal_api.py
import asyncio
import time
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

import pytest
from fastapi.testclient import TestClient

import app.api as api_mod
from app import internal_api, pqueue
from app.models import SessionInfo

SECRET = "ab" * 32
ROUTE = "/internal/ping"


@pytest.fixture(autouse=True)
def _env(monkeypatch, tmp_path):
    internal_api.set_secret(SECRET)
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: tmp_path)
    yield
    internal_api.set_secret(None)


def _client(ip="127.0.0.1"):
    return TestClient(api_mod.app, client=(ip, 50000))


def _info(**kw):
    return SessionInfo(**{"name": "s1", "cwd": "/p", "jsonl": "/p/abc-123.jsonl", "provider": "claude", **kw})


def _get(client, headers=None):
    return client.get(ROUTE, headers=headers if headers is not None else {"X-Hangar-Internal": SECRET})




def test_workspace_context_contains_only_registry_metadata(tmp_path):
    with patch("app.api._guardar_snap", return_value=[_info()]), patch("app.fs.allowed_roots", return_value=[tmp_path]):
        response = _client().get("/internal/workspace/context?name=s1", headers={"X-Hangar-Internal": SECRET})
    assert response.status_code == 200
    assert response.json() == {"roots":[str(tmp_path)], "sessions":[{"name":"s1","cwd":"/p"}],
                               "session":{"name":"s1","cwd":"/p","jsonl":"/p/abc-123.jsonl","git_cwd":"/p"}}


def test_worktrees_context_has_folders_inside_roots_and_account_projects(tmp_path):
    inside = tmp_path / "repo"
    inputs = AsyncMock(return_value=([_info(cwd=str(inside), worktree_path=str(inside))], [str(inside)], [tmp_path]))
    with patch("app.api._worktree_inputs", inputs), \
            patch("app.archive._contas", return_value=[(None, "", tmp_path / "projects")]), \
            patch("app.api.list_folders", return_value=[SimpleNamespace(cwd=str(inside / "sub"), mtime=1.0)]):
        response = _client().get("/internal/worktrees/context", headers={"X-Hangar-Internal": SECRET})
    assert response.status_code == 200
    assert response.json() == {
        "roots": [str(tmp_path)], "cwds": [str(inside)],
        "sessions": [{"name": "s1", "cwd": str(inside), "git_cwd": str(inside), "worktree_path": str(inside), "jsonl": "/p/abc-123.jsonl"}],
        "project_bases": [str(tmp_path / "projects")],
        "activities": [{"cwd": str(inside / "sub"), "mtime": 1.0}],
    }


def test_worktrees_context_refuses_before_reading_anything():
    with patch("app.api._worktree_inputs") as inputs:
        response = _client("203.0.113.7").get("/internal/worktrees/context", headers={"X-Hangar-Internal": SECRET})
        assert _client().get("/internal/worktrees/context", headers={"X-Hangar-Internal": "errado"}).status_code == 404
    assert response.status_code == 404
    inputs.assert_not_called()


def test_worktree_inputs_keep_recent_folders_inside_the_roots(tmp_path):
    (tmp_path / "repo").mkdir()
    sessions = [_info(cwd=str(tmp_path / "repo")), _info(name="s2", cwd="/fora")]
    folders = [SimpleNamespace(cwd=str(tmp_path), mtime=0), SimpleNamespace(cwd="/fora-recente", mtime=time.time()),
               SimpleNamespace(cwd=str(tmp_path / "recente"), mtime=time.time())]
    with patch("app.api.registry.list", return_value=sessions), patch("app.api.list_folders", return_value=folders), \
            patch("app.api.allowed_roots", return_value=[tmp_path]):
        _sessions, cwds, roots = asyncio.run(api_mod._worktree_inputs())
    assert cwds == [str(tmp_path / "repo"), str(tmp_path / "recente")] and roots == [tmp_path]


def test_workspace_context_refuses_before_reading_the_registry():
    with patch("app.api._guardar_snap") as snapshot:
        response = _client("203.0.113.7").get("/internal/workspace/context", headers={"X-Hangar-Internal":SECRET})
    assert response.status_code == 404
    snapshot.assert_not_called()


@pytest.mark.parametrize("headers", [{}, {"X-Hangar-Internal": "errado"}, {"X-Hangar-Internal": ""}])
def test_wrong_secret_404(headers):
    assert _get(_client(), headers=headers).status_code == 404


def test_no_secret_404():
    internal_api.set_secret(None)
    assert _get(_client(), headers={"X-Hangar-Internal": ""}).status_code == 404


@pytest.fixture
def events(monkeypatch):
    got = []
    # Só o evento desta rota: o middleware também registra cada 404 como `api.servidor`.
    monkeypatch.setattr(internal_api.diag, "registrar",
                        lambda evento, nivel="ok", **campos: evento == "internal.recusado"
                        and got.append((evento, nivel, campos)))
    return got


def test_refused_hangar_server_goes_to_the_diary_without_the_secret(events):
    assert _get(_client(), headers={"X-Hangar-Internal": "errado"}).status_code == 404
    internal_api.set_secret(None)
    assert _get(_client(), headers={"X-Hangar-Internal": SECRET}).status_code == 404
    assert events == [("internal.recusado", "aviso", {"codigo": "segredo_errado"}),
                      ("internal.recusado", "aviso", {"codigo": "sem_segredo"})]
    assert SECRET not in repr(events)


def test_requests_that_are_not_the_hangar_server_stay_out_of_the_diary(events):
    assert _get(_client(), headers={}).status_code == 404              # sem cabeçalho
    assert _get(_client("10.0.0.5"), headers={"X-Hangar-Internal": "x"}).status_code == 404   # de fora
    assert _get(_client()).status_code == 200                          # aceito
    assert events == []


def test_secret_never_goes_to_environ(monkeypatch):
    monkeypatch.delenv("HANGAR_INTERNAL_SECRET", raising=False)
    internal_api.set_secret(SECRET)
    import os
    assert "HANGAR_INTERNAL_SECRET" not in os.environ
    assert _get(_client()).status_code == 200


def test_outside_loopback_404_even_with_secret():
    assert _get(_client("10.0.0.7")).status_code == 404


def test_left_out_of_the_api_schema():
    paths = api_mod.app.openapi()["paths"]
    assert "/api/sessions/{name}/history" in paths
    assert not any(p.startswith("/internal") for p in paths)


def test_policy_route_is_gone():
    # Os serviços que o Rust pedia ao Python (sidecar, recado nativo, cálculo) são todos do Rust: a rota
    # de política não existe mais.
    assert "/internal/runtime/policy" not in {route.path for route in internal_api.router.routes}


def test_quota_route_is_gone():
    # A linha de status lê a cota no módulo de contas do Rust; aqui não sobra rota.
    assert _client().get("/internal/quota", headers={"X-Hangar-Internal": SECRET}).status_code == 404
    # As fontes de cota de fora (Kimi, CommandCode, OpenCode) também são lidas no Rust: o Rust não
    # volta ao Python dentro de um pedido de cota que o Python fez.
    assert "/internal/accounts/quotas" not in {route.path for route in internal_api.router.routes}


def test_rust_diag_route_records_event():
    body = {"evento": "rust.history_failed", "sessao": "s1", "codigo": "history_io", "motivo": "leitura falhou"}
    with patch("app.internal_api.diag.registrar") as registrar:
        ok = _client().post("/internal/diag", json=body, headers={"X-Hangar-Internal": SECRET})
        refused = _client().post("/internal/diag", json=body, headers={"X-Hangar-Internal": "errado"})
        missing = _client().post("/internal/diag", json=body)
        invalid = _client().post("/internal/diag", json={**body, "evento": "runtime.parte_para_python"},
                                 headers={"X-Hangar-Internal": SECRET})
    assert ok.status_code == 200
    assert (refused.status_code, missing.status_code) == (404, 404), "sem o segredo, nem de 127.0.0.1"
    assert invalid.status_code == 400, "o Rust só escreve eventos rust.*"
    rust_calls = [c for c in registrar.call_args_list if c.args[0].startswith("rust.")]
    assert len(rust_calls) == 1
    assert rust_calls[0].args[:2] == ("rust.history_failed", "erro")
    assert rust_calls[0].kwargs == {"sessao": "s1", "codigo": "history_io", "detalhe": "leitura falhou"}


def test_rust_diag_route_takes_the_send_events_with_the_python_level():
    base = {"sessao": "s1", "codigo": "busy", "motivo": "m"}
    with patch("app.internal_api.diag.registrar") as registrar:
        for event in ("runtime.send_failed", "runtime.send_uncertain", "runtime.command_deferred"):
            assert _client().post("/internal/diag", json={**base, "evento": event},
                                  headers={"X-Hangar-Internal": SECRET}).status_code == 200
    assert [c.args[:2] for c in registrar.call_args_list if c.args[0].startswith("runtime.")] == [
        ("runtime.send_failed", "erro"), ("runtime.send_uncertain", "aviso"), ("runtime.command_deferred", "aviso")]


def test_rust_diag_takes_codes_with_detail_suffix():
    body = {"evento": "rust.list_facts_unavailable", "sessao": "", "codigo": "list_facts_status:500", "motivo": "m"}
    with patch("app.internal_api.diag.registrar") as registrar:
        response = _client().post("/internal/diag", json=body, headers={"X-Hangar-Internal": SECRET})
    assert response.status_code == 200
    assert [c.kwargs["codigo"] for c in registrar.call_args_list if c.args[0].startswith("rust.")] == ["list_facts_status:500"]


@pytest.mark.parametrize("raw", [
    b"x" * 9000, b"not json", b"[" * 8000, b'{"evento":"rust.a","sessao":"s1","codigo":"c"}',
    b'{"evento":"rust.a","sessao":"s1","codigo":"C D","motivo":"m"}',
    b'{"evento":"rust.a","sessao":"s1","codigo":"c","motivo":1}',
])
def test_rust_diag_refuses_malformed_body(raw):
    with patch("app.internal_api.diag.registrar") as registrar:
        response = _client().post("/internal/diag", content=raw, headers={"X-Hangar-Internal": SECRET})
    assert response.status_code == 400
    assert not [c for c in registrar.call_args_list if c.args[0].startswith("rust.")]


def test_list_facts_route_takes_only_rows_and_pane_pids():
    """O pedido do Rust leva só linhas e pids; o contrato velho (contagem do dono, sombra) é recusado."""
    seen = []

    async def compute(rows, pane_pids):
        seen.append(pane_pids)
        return {"states": {}}

    body = {"rows": [], "pane_pids": {"p": 1}}
    with patch("app.list_facts.compute", compute):
        old = _client().post("/internal/list/facts", json={**body, "owner_clients": 0, "shadow": False},
                             headers={"X-Hangar-Internal": SECRET})
        wrong = _client().post("/internal/list/facts", json={**body, "pane_pids": {"p": "1"}}, headers={"X-Hangar-Internal": SECRET})
        ok = _client().post("/internal/list/facts", json=body, headers={"X-Hangar-Internal": SECRET})
    assert (old.status_code, wrong.status_code, ok.status_code) == (400, 400, 200)
    assert ok.json() == {"states": {}}
    assert seen == [{"p": 1}]


def test_busy_pool_threads_sees_uvloop_default_executor_threads():
    import threading
    from concurrent.futures import ThreadPoolExecutor
    from app.loop_monitor import _busy_pool_threads
    release, started = threading.Event(), threading.Event()
    with ThreadPoolExecutor(max_workers=1) as pool:
        pool.submit(lambda: (started.set(), release.wait(5)))
        assert started.wait(5)
        try:
            assert len(_busy_pool_threads()) == 1
        finally:
            release.set()


@pytest.mark.asyncio
async def test_stuck_pool_is_reported_before_the_probe_runs(monkeypatch):
    import threading
    from concurrent.futures import ThreadPoolExecutor
    from app import diag, loop_monitor
    monkeypatch.setattr(loop_monitor, "POOL_WAIT_WARN_S", 0.05)
    monkeypatch.setattr(loop_monitor, "POOL_STUCK_S", 0.1)
    logged = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel, **campos: logged.append((evento, campos.get("codigo"))))
    release, started = threading.Event(), threading.Event()
    with ThreadPoolExecutor(max_workers=1, thread_name_prefix="travado") as pool:
        pool.submit(lambda: (started.set(), release.wait(5)))
        assert started.wait(5)
        monitor = loop_monitor._Monitor()
        monitor.pools["travado"] = loop_monitor._Throttle()
        task = asyncio.create_task(monitor.probe_pool(asyncio.get_running_loop(), "travado", pool, ("travado",)))
        try:
            for _ in range(200):
                if logged:
                    break
                await asyncio.sleep(0.01)
            assert logged == [("pool.espera", "travado")]
            assert "travado" in monitor.probing
        finally:
            release.set()
            await task
    assert "travado" not in monitor.probing


def test_rust_que_desiste_antes_do_corpo_nao_vira_500():
    # O Rust fecha a conexão ao estourar o prazo; ler o corpo depois disso não é erro do servidor.
    sent = []

    async def receive():
        return {"type": "http.disconnect"}

    async def send(message):
        sent.append(message)

    scope = {"type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": "POST",
             "scheme": "http", "path": "/internal/list/facts", "raw_path": b"/internal/list/facts",
             "query_string": b"", "root_path": "", "client": ("127.0.0.1", 50000), "server": ("127.0.0.1", 8765),
             "headers": [(b"host", b"127.0.0.1"), (b"x-hangar-internal", SECRET.encode()),
                         (b"content-type", b"application/json"), (b"content-length", b"10")]}
    asyncio.run(api_mod.app(scope, receive, send))
    assert [m["status"] for m in sent if m["type"] == "http.response.start"] == [499]
