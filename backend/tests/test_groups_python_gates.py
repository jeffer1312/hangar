"""O que dos grupos fica no Python depois da migração: instalador do hook, ponte num sentido, leitores
da lista e o fechamento que o Rust já fez. Fecha linhas da `conferencia.md` da Task 6."""
import asyncio
import json
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import api, diag, groups_bridge, pair
from app.config import settings

H = {"Authorization": "Bearer secret"}


@pytest.fixture(autouse=True)
def _tmp_pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


@pytest.fixture
def journal(monkeypatch):
    got = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="ok", **campos: got.append((evento, nivel, campos)))
    return got


def test_pair_hook_reaches_every_account_and_the_base(tmp_path, monkeypatch):
    """HOOK-13: cada conta e a base do backend ganham o hook."""
    from app import hook_installer as hi, rust_bins
    dirs = [tmp_path / "conta-a", tmp_path / "conta-b", tmp_path / "base"]
    for d in dirs:
        d.mkdir()
        (d / "settings.json").write_text("{}")
    monkeypatch.setattr(rust_bins, "find_bin", lambda name, env: Path("/b/hangar-server"))
    monkeypatch.setattr(hi, "list_config_dirs", lambda **_: [SimpleNamespace(path=str(d)) for d in dirs[:2]])
    monkeypatch.setattr(hi, "_backend_config_base", lambda: dirs[2])
    assert sorted(hi.ensure_pair_hook_installed()) == sorted(str(d) for d in dirs)
    for d in dirs:
        [block] = json.loads((d / "settings.json").read_text())["hooks"]["SessionStart"]
        assert "pair-context" in block["hooks"][0]["command"], d


def _serve(reply):
    """Rust falso: `reply(handler)` escreve a resposta."""
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            self.rfile.read(int(self.headers["Content-Length"]))
            reply(self)

        def log_message(self, *a):
            pass
    srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def _body(raw: bytes):
    def reply(h):
        h.send_response(200)
        h.send_header("Content-Length", str(len(raw)))
        h.end_headers()
        h.wfile.write(raw)
    return reply


@pytest.mark.parametrize("case", ["grande", "sem_ok", "json_torto", "transporte"])
def test_bad_bridge_answers_are_unavailable_and_journaled(journal, case):
    """BRG-05: resposta > 4 MiB, sem `ok`, JSON torto ou conexão recusada viram código e linha no diário."""
    if case == "transporte":
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            address = f"127.0.0.1:{s.getsockname()[1]}"
        srv = None
    else:
        raw = {"grande": b'{"ok": true, "result": {"x": "' + b"a" * (4 * 1024 * 1024) + b'"}}',
               "sem_ok": b'{"result": {}}', "json_torto": b"{"}[case]
        srv = _serve(_body(raw))
        address = f"127.0.0.1:{srv.server_port}"
    groups_bridge.configure(address, "s3gredo")
    try:
        with pytest.raises(groups_bridge.GroupsBridgeError) as e:
            groups_bridge.call("group.leave", name="a")
        assert e.value.code == "groups_bridge_unavailable"
    finally:
        groups_bridge.configure(None, None)
        if srv:
            srv.shutdown()
    assert [(ev, nivel, c["operacao_rust"]) for ev, nivel, c in journal] == [("grupos.ponte", "aviso", "group.leave")]


def test_uncaught_bridge_error_is_503_with_the_journal_line(journal):
    """BRG-08: erro da ponte que escapa de uma rota vira 503 `erro_grupo_indisponivel` e `grupos.indisponivel`."""
    from starlette.requests import Request
    request = Request({"type": "http", "method": "POST", "path": "/api/sessions/a/rename", "headers": [], "query_string": b""})
    resp = asyncio.run(api._grupos_indisponiveis(request, groups_bridge.GroupsBridgeError("groups_bridge_off")))
    body = json.loads(resp.body)
    assert (resp.status_code, body["detail"]["code"], body["detail"]["params"]["detalhe"]) == (503, "erro_grupo_indisponivel", "groups_bridge_off")
    assert journal == [("grupos.indisponivel", "erro", {"codigo": "groups_bridge_off", "detalhe": "POST /api/sessions/a/rename"})]


def test_close_done_by_the_rust_never_touches_the_group(monkeypatch):
    """BRG-26: fechada pelo Rust (`x-hangar-rust-closed`), o Python não chama a ponte nem mexe no arquivo de grupo."""
    from app import internal_api
    monkeypatch.setattr(settings, "auth_token", "secret")
    side = pair._pair_dir() / "a.json"
    side.write_text(json.dumps({"peers": ["b"], "task": "", "gid": "g1", "harness": {}}), encoding="utf-8")
    before = (side.read_bytes(), side.stat().st_mtime_ns)
    monkeypatch.setattr(groups_bridge, "rust_owns_groups", lambda: True)

    def forbidden(op, **args):
        pytest.fail(f"a ponte não pode ser chamada num fechamento do Rust: {op}")
    monkeypatch.setattr(groups_bridge, "call", forbidden)
    monkeypatch.setattr(api.registry, "kill", lambda n, closed_by_rust=False: [])
    monkeypatch.setattr(api, "_invalidate_lists", lambda: None)
    monkeypatch.setattr(api.share_bridge, "revoke_session", lambda n: None)
    internal_api.set_secret("s3gredo")
    try:
        r = TestClient(api.app).delete("/api/sessions/a", headers={**H, "x-hangar-rust-closed": "s3gredo"})
    finally:
        internal_api.set_secret(None)
    assert r.status_code == 200, r.text
    assert (side.read_bytes(), side.stat().st_mtime_ns) == before


def test_python_mode_list_reads_the_group_and_a_broken_outside_registry_is_empty(tmp_path, monkeypatch):
    """BRG-29: a lista do modo `python` lê o arquivo de grupo; `external_pairs.json` torto vale vazio."""
    from app import registry as R
    from app.adapters.claude_headless import sessions as S
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(R.tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(R.codex_sessions, "list_all", lambda include_incomplete=False: [])
    monkeypatch.setattr(R, "_decorate_transfers", lambda out: None)
    S.save("s", str(tmp_path), "22222222-2222-2222-2222-222222222222")
    (pair._pair_dir() / "s.json").write_text(json.dumps(
        {"peers": ["b", "ana::y"], "task": "T-1", "gid": "ab12cd34", "harness": {}}), encoding="utf-8")
    registry_file = pair._pair_dir() / "external_pairs.json"
    registry_file.write_text("{", encoding="utf-8")
    reg = R.SessionRegistry(str(tmp_path / "projects"))
    [row] = [r for r in reg.list() if r.name == "s"]
    assert (row.pair_peers, row.pair_gid, row.pair_task, row.pair_external) == (["b", "ana::y"], "ab12cd34", "T-1", None)
    registry_file.write_text(json.dumps([{"local_session": "s", "alias": "ana", "peer_session": "y", "peer_owner": "Ana"}]))
    [row] = [r for r in reg.list() if r.name == "s"]
    assert row.pair_external == {"alias": "ana", "owner": "Ana", "session": "y"}
