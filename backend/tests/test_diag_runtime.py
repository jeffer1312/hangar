"""O arquivo exportado explica a falha sem carregar conteúdo do terminal."""
import asyncio
import json
import subprocess
from concurrent.futures import ThreadPoolExecutor
from threading import Event, local
from types import SimpleNamespace

import pytest

from app import diag, terminal_input, tmux

VERSION_PROBE = diag._versao_mux


@pytest.fixture(autouse=True)
def isolated_diary(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    monkeypatch.setattr(diag, "_versao_mux", lambda: "psmux 3.3.7", raising=False)
    monkeypatch.setattr(terminal_input, "_ULTIMA_LIMPEZA", local())


def events():
    return [json.loads(line) for line in diag.ler_tudo().splitlines()]


@pytest.mark.parametrize("failure,kind,code", [
    (subprocess.TimeoutExpired(["tmux", "segredo"], 5, output="segredo"), "TimeoutExpired", 240),
    (FileNotFoundError(2, "segredo"), "FileNotFoundError", 240),
    (PermissionError(13, "segredo"), "PermissionError", 240),
    (None, "retorno", 7),
])
def test_command_failure_exports_safe_cause(monkeypatch, failure, kind, code):
    def run(*args, **kwargs):
        if failure:
            raise failure
        return subprocess.CompletedProcess(args, 7, b"segredo", b"segredo")

    monkeypatch.setattr(tmux, "RUN", run)
    token = diag.req_atual.set("aba-123")
    try:
        result = tmux._run(["tmux", "send-keys", "-t", "=sessao:", "segredo"], input=b"segredo")
    finally:
        diag.req_atual.reset(token)
    assert result.returncode == code
    row = next(r for r in events() if r["evento"] == "mux.comando")
    assert row["comando"] == "send-keys"
    assert row["erro_tipo"] == kind and row["retorno"] == code
    assert row["limite_ms"] == 5000 and row["ms"] >= 0
    assert row["simultaneos"] == 1 and row["req"] == "aba-123"
    assert row["backend"] == diag.VERSAO_EM_EXECUCAO
    if failure and isinstance(failure, OSError):
        assert row["errno"] == failure.errno
    assert "segredo" not in diag.ler_tudo()


def test_command_concurrency_and_counter_cleanup(monkeypatch):
    started, release = Event(), Event()

    def run(args, **kwargs):
        if args[1] == "list-panes":
            started.set()
            assert release.wait(3)
            return subprocess.CompletedProcess(args, 0, "", "")
        raise subprocess.TimeoutExpired(args, 5)

    monkeypatch.setattr(tmux, "RUN", run)
    with ThreadPoolExecutor(max_workers=1) as pool:
        pending = pool.submit(tmux._run, ["tmux", "list-panes"])
        try:
            assert started.wait(3)
            tmux._run(["tmux", "capture-pane"])
        finally:
            release.set()
        pending.result()
    tmux._run(["tmux", "capture-pane"])
    failures = [r for r in events() if r.get("erro_tipo") == "TimeoutExpired"]
    assert [r["simultaneos"] for r in failures] == [2, 1]


def test_normal_absence_does_not_fill_diary(monkeypatch):
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: subprocess.CompletedProcess(args, 1, "", "no server"))
    assert tmux.list_panes_all() == {}
    assert tmux.has_session("gone") is False
    assert not any(r["evento"] == "mux.comando" for r in events())


def test_other_list_failure_is_not_hidden_as_absence(monkeypatch):
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: subprocess.CompletedProcess(args, 1, "", "permission denied: segredo"))
    tmux.list_panes_all()
    row = next(r for r in events() if r["evento"] == "mux.comando")
    assert row["retorno"] == 1 and row["erro_tipo"] == "retorno" and row["nivel"] == "erro"
    assert "segredo" not in diag.ler_tudo()


def test_partial_exports_stage_and_cleanup_without_composer(monkeypatch):
    pane = "\n".join(["─" * 30, "❯ segredo [Pasted text #3 +2 lines]", "─" * 30, "? for shortcuts"])
    monkeypatch.setattr(terminal_input, "_capture", lambda name: pane)
    monkeypatch.setattr(terminal_input, "_limpar_composer", lambda *args: False)
    text = "segredo\nsegunda linha"
    assert terminal_input._partial("sessao", "sem prova de colagem", text, {"1"},
                                   etapa="clipboard.prova", provider="claude") == "partial"
    row = next(r for r in events() if r["evento"] == "envio.parcial")
    assert row["etapa"] == "clipboard.prova" and row["provider"] == "claude"
    assert row["caracteres"] == len(text) and row["linhas"] == 2
    assert row["composer_legivel"] is True and row["limpou"] is False
    assert row["pastes_antes"] == 1 and row["pastes_depois"] == 1
    assert "segredo" not in diag.ler_tudo() and "segunda linha" not in diag.ler_tudo()


def test_send_thread_preserves_request_context():
    from app.api import _send_thread

    async def run():
        token = diag.req_atual.set("aba-envio")
        try:
            await _send_thread(diag.registrar, "teste.envio")
        finally:
            diag.req_atual.reset(token)

    asyncio.run(run())
    row = next(r for r in events() if r["evento"] == "teste.envio")
    assert row["req"] == "aba-envio"


def test_export_identifies_current_environment():
    header = events()[0]
    assert header["mux"] == "psmux 3.3.7"
    assert header["python"] and header["sistema"] and header["arquitetura"]
    assert header["inicio_backend"] and header["pid_backend"] > 0


def _git_falso(respostas):
    """`respostas` mapeia o 1º argumento depois de `git` (`rev-list`, `show`) pra stdout."""
    def run(args, **kw):
        return subprocess.CompletedProcess(args, 0, respostas.get(args[1], ""), "")
    return run


def test_versao_legivel_e_version_mais_contagem(monkeypatch):
    monkeypatch.setattr(diag.subprocess, "run", _git_falso({"rev-list": "2533\n", "show": "0.1.0\n"}))
    assert diag.versao_legivel() == "0.1.0.2533"


def test_versao_legivel_ref_inexistente_e_none(monkeypatch):
    monkeypatch.setattr(diag.subprocess, "run", _git_falso({}))
    assert diag.versao_legivel("nao-existe") is None


def test_versao_legivel_remoto_sem_version_nao_inventa(monkeypatch):
    """origin/main sem o arquivo: juntar o VERSION daqui com a contagem de lá seria uma versão
    plausível e falsa. Só o HEAD pode cair no disco."""
    monkeypatch.setattr(diag.subprocess, "run", _git_falso({"rev-list": "2521\n"}))
    assert diag.versao_legivel("origin/main") is None
    assert diag.versao_legivel("HEAD") is not None      # o disco tem VERSION


def test_missing_git_version_is_explicit(monkeypatch):
    monkeypatch.setattr(diag.subprocess, "run", lambda *args, **kw: subprocess.CompletedProcess(args, 1, "", ""))
    assert diag._git_describe() == "indisponivel"


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


def test_slow_success_is_reported_without_output(monkeypatch):
    clock = iter([10.0, 11.2])
    monkeypatch.setattr(tmux.time, "monotonic", lambda: next(clock))
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: subprocess.CompletedProcess(args, 0, "segredo", ""))
    assert tmux._run(["tmux", "capture-pane"]).stdout == "segredo"
    row = next(r for r in events() if r["evento"] == "mux.comando")
    assert row["nivel"] == "aviso" and row["retorno"] == 0 and row["ms"] >= 1000
    assert "segredo" not in diag.ler_tudo()


@pytest.mark.parametrize("stage", ["sessao_windows", "escrita", "tecla_colar"])
def test_clipboard_exports_real_failure_stage(monkeypatch, stage):
    monkeypatch.setattr(tmux, "pane_pid", lambda name: 999)
    ids = iter([0, 2 if stage == "sessao_windows" else 0])
    monkeypatch.setattr(tmux, "_sessao_windows_de", lambda pid: next(ids))
    monkeypatch.setattr(tmux, "RUN", lambda args, **kw: subprocess.CompletedProcess(args, 7 if stage == "escrita" else 0, b"", b"segredo"))
    monkeypatch.setattr(tmux, "send_keys", lambda *args: False)
    # os.name altera também pathlib; a pasta do diário já é um Path do host real.
    base = diag._base()
    monkeypatch.setattr(diag, "_base", lambda: base)
    # A trava do clipboard também mora numa pasta do host: resolvida antes de fingir Windows.
    from app import runtime_terminal
    lock = runtime_terminal._clipboard_lock_path()
    monkeypatch.setattr(runtime_terminal, "_clipboard_lock_path", lambda: lock)
    with monkeypatch.context() as win:
        win.setattr(tmux.os, "name", "nt")
        assert tmux.paste_via_clipboard("sessao", "segredo") is False
    row = next(r for r in events() if r["evento"] == "envio.clipboard")
    assert row["etapa"] == stage
    assert "segredo" not in diag.ler_tudo()


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


@pytest.mark.parametrize("status", [200, 409])
def test_plugin_long_poll_logs_only_failures(status):
    from app.api import _correlaciona_diag
    from starlette.requests import Request
    from starlette.responses import Response

    request = Request({"type": "http", "method": "POST", "path": "/api/plugin/pull",
                       "query_string": b"", "headers": [],
                       "route": SimpleNamespace(path="/api/plugin/pull")})

    async def respond(req):
        return Response("", status_code=status)

    asyncio.run(_correlaciona_diag(request, respond))
    rows = [r for r in events() if r["evento"] == "api.servidor"]
    assert [r["codigo"] for r in rows] == ([] if status == 200 else ["409"])


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


MANIFEST = {"version": "0.1.0.8207", "commit": "abc1234def5678", "channel": "beta",
            "protocol": 1, "api": 1, "os": "linux-x86_64", "dist_key": None, "files": {}}


def _sem_git(*args, **kw):
    raise AssertionError("pacote instalado não chama git")


def test_installed_package_takes_version_from_manifest_without_git(monkeypatch, tmp_path):
    (tmp_path / "manifest.json").write_text(json.dumps(MANIFEST), encoding="utf-8")
    monkeypatch.setattr(diag, "_ROOT", tmp_path)
    monkeypatch.setattr(diag.subprocess, "run", _sem_git)
    assert diag.versao_legivel() == "0.1.0.8207"
    assert diag._git_describe() == "0.1.0.8207-gabc1234def56"
    assert diag.install_manifest()["channel"] == "beta"


def test_without_manifest_version_comes_from_git(monkeypatch, tmp_path):
    monkeypatch.setattr(diag, "_ROOT", tmp_path)
    monkeypatch.setattr(diag.subprocess, "run", _git_falso({"rev-list": "2533\n", "show": "0.1.0\n"}))
    assert diag.install_manifest() is None
    assert diag.versao_legivel() == "0.1.0.2533"


@pytest.mark.parametrize("conteudo", ["{", "[]", json.dumps({**MANIFEST, "channel": ""}),
                                      json.dumps({k: v for k, v in MANIFEST.items() if k != "commit"})])
def test_malformed_manifest_falls_back_to_git_and_logs(monkeypatch, tmp_path, caplog, conteudo):
    (tmp_path / "manifest.json").write_text(conteudo, encoding="utf-8")
    monkeypatch.setattr(diag, "_ROOT", tmp_path)
    monkeypatch.setattr(diag.subprocess, "run", _git_falso({"rev-list": "2533\n", "show": "0.1.0\n"}))
    with caplog.at_level("WARNING", logger="hangar.diag"):
        assert diag.versao_legivel() == "0.1.0.2533"
    assert diag.install_manifest() is None
    assert any("manifest.json" in r.getMessage() for r in caplog.records)
