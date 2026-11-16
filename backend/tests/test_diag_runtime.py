"""O arquivo exportado explica a falha sem carregar conteúdo do terminal."""
import asyncio
import json
import subprocess
from types import SimpleNamespace

import pytest

from app import diag

VERSION_PROBE = diag._versao_mux


@pytest.fixture(autouse=True)
def isolated_diary(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    monkeypatch.setattr(diag, "_versao_mux", lambda: "psmux 3.3.7", raising=False)


def events():
    return [json.loads(line) for line in diag.ler_tudo().splitlines()]


def test_export_identifies_current_environment():
    header = events()[0]
    assert header["mux"] == "psmux 3.3.7"
    assert header["python"] and header["sistema"] and header["arquitetura"]
    assert header["inicio_backend"] and header["pid_backend"] > 0




@pytest.mark.parametrize("output,code,expected", [
    ("psmux 3.3.7\n", 0, "psmux 3.3.7"),
    ("tmux 3.6a\n", 0, "tmux 3.6a"),
    ("segredo\npsmux 3.3.7", 0, "resposta_nao_reconhecida"),
    ("segredo", 1, "retorno:1"),
])
def test_version_probe_never_exports_arbitrary_output(monkeypatch, output, code, expected):
    monkeypatch.setattr(diag.subprocess, "run", lambda *a, **kw: subprocess.CompletedProcess(a, code, output, "segredo"))
    assert VERSION_PROBE() == expected


def test_windows_memory_snapshot_and_failure(monkeypatch):
    import psutil
    monkeypatch.setattr(diag.sys, "platform", "win32")
    monkeypatch.setattr(psutil, "virtual_memory", lambda: SimpleNamespace(total=8 * 1024**3, available=1024**3))
    assert diag.recursos()["memoria_total_mb"] == 8192
    assert diag.recursos()["memoria_disponivel_mb"] == 1024

    def denied():
        raise psutil.AccessDenied()

    monkeypatch.setattr(psutil, "virtual_memory", denied)
    assert diag.recursos()["recursos_erro"] == "AccessDenied"


def test_http_failure_exports_server_duration_and_safe_route():
    from app.api import _correlaciona_diag
    from starlette.requests import Request
    from starlette.responses import Response

    request = Request({"type": "http", "method": "POST", "path": "/api/sessions/sessao/input",
                       "query_string": b"token=segredo", "headers": [(b"x-hangar-req", b"aba-http")],
                       "route": SimpleNamespace(path="/api/sessions/{name}/input"),
                       "path_params": {"name": "sessao"}})

    async def respond(req):
        return Response("segredo", status_code=400)

    response = asyncio.run(_correlaciona_diag(request, respond))
    assert response.status_code == 400
    row = next(r for r in events() if r["evento"] == "api.servidor")
    assert row["req"] == "aba-http" and row["codigo"] == "400" and row["ms"] >= 0
    assert row["detalhe"] == "POST /api/sessions/{name}/input"
    assert row["sessao"] == "sessao"
    assert diag.req_atual.get() == ""
    assert "segredo" not in diag.ler_tudo()


@pytest.mark.parametrize("status", [200, 500])
def test_internal_rust_calls_log_only_failures(status):
    from app.api import _correlaciona_diag
    from starlette.requests import Request
    from starlette.responses import Response

    request = Request({"type": "http", "method": "POST", "path": "/internal/list/facts",
                       "query_string": b"", "headers": [],
                       "route": SimpleNamespace(path="/internal/list/facts")})

    async def respond(req):
        return Response("", status_code=status)

    asyncio.run(_correlaciona_diag(request, respond))
    rows = [r for r in events() if r["evento"] == "api.servidor"]
    assert [r["codigo"] for r in rows] == ([] if status == 200 else ["500"])


@pytest.mark.parametrize("failure", [RuntimeError("segredo"), asyncio.CancelledError()])
def test_http_exception_and_cancellation_are_distinct(failure):
    from app.api import _correlaciona_diag
    from starlette.requests import Request

    request = Request({"type": "http", "method": "GET", "path": "/segredo",
                       "query_string": b"", "headers": [(b"x-hangar-req", b"aba-falha")]})

    async def respond(req):
        raise failure

    with pytest.raises(type(failure)):
        asyncio.run(_correlaciona_diag(request, respond))
    rows = [r for r in events() if r["evento"] == "api.servidor"]
    if isinstance(failure, asyncio.CancelledError):
        assert rows == []
    else:
        assert rows[0]["erro_tipo"] == "RuntimeError" and rows[0]["req"] == "aba-falha"
    assert "segredo" not in diag.ler_tudo()
