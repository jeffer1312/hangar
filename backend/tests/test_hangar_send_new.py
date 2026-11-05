"""`hangar-send --new`: o corpo inteiro do pedido de abertura e o nome que o servidor devolveu."""
import json
import shutil
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import pytest


@pytest.fixture
def backend(tmp_path, hangar_server_bin):
    requests, paths = [], []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'{"claude": {"disponivel": true, "default": true}}')

        def do_POST(self):
            paths.append(self.path)
            requests.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            self.send_response(200)
            self.end_headers()
            self.wfile.write(json.dumps({"name": "Area-de-trabalho", "cwd": "/tmp"}).encode())

        def log_message(self, *args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    (tmp_path / "scripts").mkdir()
    (tmp_path / "backend").mkdir()
    (tmp_path / "backend" / ".env").write_text(f"CP_AUTH_TOKEN=test\nCP_PORT={server.server_port}\n")
    script = tmp_path / "scripts" / "hangar-send"
    shutil.copy(Path(__file__).parents[2] / "scripts" / "hangar-send", script)
    shutil.copy(Path(__file__).parents[2] / "scripts" / "hangar-server-command", script.parent / "hangar-server-command")
    env = {"PATH": "/usr/bin:/bin", "HOME": str(tmp_path), "USERPROFILE": str(tmp_path),
           "CP_RUST_SERVER_BIN": str(hangar_server_bin)}

    def run(*args):
        return subprocess.run(["bash", str(script), "--new", *args], capture_output=True, text=True, timeout=10, env=env)

    run.paths = paths
    yield run, requests
    server.shutdown()
    server.server_close()
    worker.join()


def test_prints_the_name_the_server_gave(backend):
    run, _ = backend
    result = run("Área de trabalho", "/tmp")
    assert result.returncode == 0, result.stderr
    assert "sessão criada: Area-de-trabalho (/tmp)" in result.stdout
    assert 'hangar-send Area-de-trabalho "msg"' in result.stdout


def test_sends_every_open_field(backend):
    run, requests = backend
    custom = '{"plugins": {"x@y": false}}'
    result = run("s1", "/tmp", "--no-inherit", "--engine", "gpt", "--engine-account", "conta-1",
                 "--branch", "feat-x", "--new-branch", "--base", "main", "--customizations", custom,
                 "--prompt", "olá")
    assert result.returncode == 0, result.stderr
    body = requests[-1]
    assert body["inherit"] is False
    assert body["engine_account"] == "conta-1"
    assert (body["branch"], body["new_branch"], body["base"]) == ("feat-x", True, "main")
    assert body["claude_customizations"] == {"plugins": {"x@y": False}}
    assert body["initial_prompt"] == "olá"


def test_omitted_fields_stay_absent(backend):
    run, requests = backend
    assert run("s1", "/tmp").returncode == 0
    body = requests[-1]
    for key in ("inherit", "engine_account", "branch", "new_branch", "base", "claude_customizations", "initial_prompt"):
        assert key not in body


@pytest.mark.parametrize("args", [("--base", "main"), ("--new-branch",), ("--customizations", "{x")])
def test_incoherent_branch_or_bad_json_is_a_usage_error(backend, args):
    run, requests = backend
    result = run("s1", "/tmp", *args)
    assert result.returncode == 2
    assert not requests


def test_terminal_with_a_pane_provider_is_accepted(backend):
    run, requests = backend
    result = run("s1", "/tmp", "--terminal", "--provider", "pi")
    assert result.returncode == 0, result.stderr
    assert requests[-1]["provider"] == "pi"
    assert requests[-1]["headless"] is False


SID = "11111111-1111-4111-8111-111111111111"


def test_resume_brings_back_a_conversation_through_the_archive_route(backend):
    run, requests = backend
    result = run("volta", "--resume", SID, "--terminal")
    assert result.returncode == 0, result.stderr
    assert run.paths[-1] == f"/api/archive/-/{SID}/resume"
    assert requests[-1] == {"name": "volta", "provider": "claude", "headless": False}
    assert "sessão retomada: Area-de-trabalho" in result.stdout


def test_resume_refuses_what_the_resume_does_not_take(backend):
    run, requests = backend
    result = run("volta", "--resume", SID, "--permissao", "plan")
    assert result.returncode == 2 and "--permissao" in result.stderr
    assert not requests
