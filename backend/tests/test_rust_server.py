# backend/tests/test_rust_server.py
"""main.py com o hangar-server: sobe, espera a saúde, religa, e o Python assume a porta pública.

O binário é um falso em Python que escuta a porta pública e responde a saúde como o de verdade,
e sai quando o stdin fecha, também como o de verdade; o modo (`FAKE_MODE`) decide se ele fica
(`ok`), cai logo depois de subir (`cai`) ou nunca responde (`mudo`), e `FAKE_PROTOCOL` muda o
`protocol` da saúde (`sem` = campo ausente). Linux só: o falso é um script com shebang e a morte
do filho é lida em /proc.
"""
import asyncio
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import pytest
import uvicorn

from app import diag, internal_api, main, rust_server
from app.config import Settings

pytestmark = pytest.mark.skipif(not sys.platform.startswith("linux"),
                                reason="binário falso com shebang e /proc")

FAKE = r'''
import json, os, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(json.dumps({"pid": os.getpid(), "listen": os.environ["HANGAR_SERVER_LISTEN"],
                        "upstream": os.environ["HANGAR_SERVER_UPSTREAM"],
                        "secret": os.environ["HANGAR_INTERNAL_SECRET"],
                        "token": os.environ["CP_AUTH_TOKEN"],
                        "forwarded": os.environ["CP_FORWARDED_ALLOW_IPS"],
                        "log": os.environ["HANGAR_SERVER_LOG"]}) + "\n")


def parent_gone():
    # Igual ao binário: o Python segura o cano do stdin; fechou = pai morreu.
    while os.read(0, 64):
        pass
    os._exit(0)


threading.Thread(target=parent_gone, daemon=True).start()
mode = os.environ.get("FAKE_MODE", "ok")
if mode == "mudo":
    time.sleep(60)
    sys.exit(0)
protocol = os.environ.get("FAKE_PROTOCOL", "__PROTOCOL__")


class Health(BaseHTTPRequestHandler):
    def do_GET(self):
        health = {"ok": True, "version": "0.0.0-test",
                  "terminal_address": f"127.0.0.1:{self.server.server_port}"}
        if protocol != "sem":
            health["protocol"] = int(protocol)
        body = json.dumps(health).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


host, port = os.environ["HANGAR_SERVER_LISTEN"].rsplit(":", 1)
server = ThreadingHTTPServer((host, int(port)), Health)
if mode == "cai":
    threading.Timer(0.3, lambda: os._exit(1)).start()
server.serve_forever()
'''


class _App:
    """ASGI mínimo: conta as subidas do lifespan e responde "python" em qualquer rota."""

    def __init__(self):
        self.startups = 0

    async def __call__(self, scope, receive, send):
        if scope["type"] == "lifespan":
            while True:
                message = await receive()
                if message["type"] == "lifespan.startup":
                    self.startups += 1
                    await send({"type": "lifespan.startup.complete"})
                elif message["type"] == "lifespan.shutdown":
                    await send({"type": "lifespan.shutdown.complete"})
                    return
        await send({"type": "http.response.start", "status": 200,
                    "headers": [(b"content-type", b"text/plain")]})
        await send({"type": "http.response.body", "body": b"python"})


@pytest.fixture
def fake_bin(tmp_path, monkeypatch):
    path = tmp_path / "hangar-server"
    source = FAKE.replace("__PROTOCOL__", str(rust_server.RUST_SERVER_PROTOCOL))
    path.write_text(f"#!{sys.executable}\n{source}", encoding="utf-8")
    path.chmod(0o755)
    monkeypatch.setenv("FAKE_LOG", str(tmp_path / "spawns.jsonl"))
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.delenv("HANGAR_INTERNAL_SECRET", raising=False)
    monkeypatch.setattr(rust_server, "_POLL", 0.05)
    yield path
    internal_api.set_secret(None)


@pytest.fixture
def events(monkeypatch):
    got = []
    monkeypatch.setattr(diag, "registrar",
                        lambda evento, nivel="ok", **campos: got.append((evento, nivel, campos)))
    return got


@pytest.fixture
def relog(monkeypatch):
    calls = []
    monkeypatch.setattr(rust_server.diag_logging, "instalar", lambda *a: calls.append(a))
    return calls


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _setup(app, public_port):
    kw = dict(host="127.0.0.1", port=public_port, workers=1, proxy_headers=True,
              forwarded_allow_ips="10.0.0.2", log_level="warning")
    server = uvicorn.Server(uvicorn.Config(app, **kw))
    return server, main._tcp_socket("127.0.0.1", 0), kw


def _bind(port):
    return lambda: main._tcp_socket("127.0.0.1", port)


def _spawns(tmp_path) -> list[dict]:
    log = tmp_path / "spawns.jsonl"
    return [json.loads(l) for l in log.read_text().splitlines()] if log.exists() else []


def _get(port: int, path: str) -> bytes:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(f"http://127.0.0.1:{port}{path}", timeout=1) as r:
        return r.read()


async def _wait_get(port: int, path: str, accept, timeout: float = 15.0) -> bytes:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            body = await asyncio.to_thread(_get, port, path)
            if accept(body):
                return body
        except OSError:
            pass
        await asyncio.sleep(0.05)
    raise AssertionError(f"a porta {port} não respondeu {path}")


def _dead(pid: int) -> bool:
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().rsplit(") ", 1)[1].startswith("Z")
    except (FileNotFoundError, ProcessLookupError):   # o segundo: colhido entre o open e o read
        return True


def _proc_info(pid: int, tmp_path: Path) -> str:
    # Diagnóstico temporário do CI: quem é o pid, quem é o pai e para onde apontam os fds.
    def read(path):
        try:
            return Path(path).read_text(errors="replace").replace("\0", " ")
        except OSError as e:
            return repr(e)
    status = read(f"/proc/{pid}/status")
    ppid = next((l.split()[1] for l in status.splitlines() if l.startswith("PPid:")), "?")
    fds = {}
    for fd in ("0", "1", "2"):
        try:
            fds[fd] = os.readlink(f"/proc/{pid}/fd/{fd}")
        except OSError as e:
            fds[fd] = repr(e)
    return "\n".join([status[:400], "cmdline=" + read(f"/proc/{pid}/cmdline"), f"fds={fds}",
                      f"parent {ppid}: " + read(f"/proc/{ppid}/cmdline"),
                      "log=" + read(tmp_path / "spawns.jsonl")[:300]])


def test_child_takes_public_port_and_gets_the_contract_env(fake_bin, tmp_path, events):
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)
    upstream = internal.getsockname()[1]

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, rust_server.HEALTH_PATH, lambda b: json.loads(b)["ok"] is True)
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    [spawn] = _spawns(tmp_path)
    assert spawn["listen"] == f"127.0.0.1:{port}"
    assert spawn["upstream"] == f"127.0.0.1:{upstream}"
    assert spawn["token"] == "tok"
    assert spawn["forwarded"] == "10.0.0.2"         # a lista do dono vale na porta pública
    assert re.fullmatch(r"[0-9a-f]{64}", spawn["secret"])
    assert internal_api._secret == spawn["secret"]
    assert "HANGAR_INTERNAL_SECRET" not in os.environ   # nunca vaza para as sessões do backend
    assert spawn["log"] == str(rust_server.server_log_path())   # o conftest isola o log_paths.base()
    assert spawn["log"].endswith(os.path.join("privado", "hangar-server.log"))
    assert _dead(spawn["pid"])                      # o Python leva o filho junto ao sair
    assert ("hangar_server.de_pe", "ok", {}) in events
    assert app.startups == 1


def test_three_crashes_in_a_minute_hand_the_public_port_to_python(
        fake_bin, tmp_path, monkeypatch, events, relog):
    monkeypatch.setenv("FAKE_MODE", "cai")
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, "/", lambda b: b == b"python")
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    spawns = _spawns(tmp_path)
    assert len(spawns) == rust_server.MAX_CRASHES
    assert all(_dead(s["pid"]) for s in spawns)
    assert len({s["secret"] for s in spawns}) == rust_server.MAX_CRASHES   # segredo novo a cada subida
    assert [e for e in events if e[0] == "hangar_server.caiu"]
    assert ("hangar_server.reserva", "erro", {"codigo": "quedas"}) in events
    assert app.startups == 1                        # a porta pública não rodou o lifespan de novo
    assert relog                                    # o diário voltou a ouvir o uvicorn


def test_child_that_never_answers_is_killed_and_python_takes_over(
        fake_bin, tmp_path, monkeypatch, events, relog):
    monkeypatch.setenv("FAKE_MODE", "mudo")
    monkeypatch.setattr(rust_server, "START_TIMEOUT", 1.0)
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, "/", lambda b: b == b"python")
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    [spawn] = _spawns(tmp_path)
    assert _dead(spawn["pid"])
    assert ("hangar_server.reserva", "erro", {"codigo": "sem_resposta"}) in events


def test_public_port_taken_at_takeover_ends_the_process_with_failure(
        fake_bin, monkeypatch, events, relog):
    monkeypatch.setenv("FAKE_MODE", "cai")
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    def occupied():
        raise OSError(98, "Address already in use")

    assert asyncio.run(
        rust_server.serve(server, [internal], fake_bin, kw, "tok", occupied)) is False
    assert any(e[0] == "hangar_server.reserva" and e[2].get("codigo") == "porta_ocupada"
               for e in events)


_CHILD_ENV = dict(HANGAR_SERVER_LISTEN="127.0.0.1:1", HANGAR_SERVER_UPSTREAM="127.0.0.1:2",
                  HANGAR_INTERNAL_SECRET="s", CP_AUTH_TOKEN="t", CP_FORWARDED_ALLOW_IPS="127.0.0.1",
                  HANGAR_SERVER_LOG="l")


def test_closing_stdin_makes_the_child_exit(fake_bin, monkeypatch):
    # `mudo` dormiria 60 s: sair em 5 s com código 0 só pode ser o fim do cano.
    monkeypatch.setenv("FAKE_MODE", "mudo")
    proc = rust_server._spawn(fake_bin, {**os.environ, **_CHILD_ENV})
    assert proc.poll() is None
    proc.stdin.close()
    assert proc.wait(timeout=5) == 0


def test_child_dies_when_python_is_killed(fake_bin, tmp_path, monkeypatch):
    """O cano do stdin: um Python morto a SIGKILL fecha a ponta dele, e o filho sai."""
    monkeypatch.setenv("FAKE_MODE", "mudo")
    code = ("import json, os, sys\n"
            "from pathlib import Path\n"
            "from app import rust_server\n"
            "env = dict(os.environ, **json.loads(sys.argv[2]))\n"
            "p = rust_server._spawn(Path(sys.argv[1]), env)\n"
            "print(p.pid, flush=True)\n"
            "os.kill(os.getpid(), 9)\n")
    t0 = time.monotonic()
    out = subprocess.run([sys.executable, "-c", code, str(fake_bin), json.dumps(_CHILD_ENV)],
                         cwd=Path(rust_server.__file__).resolve().parents[1],
                         capture_output=True, text=True, timeout=60)
    pid = int(out.stdout.split()[0])
    deadline = time.monotonic() + 5
    while not _dead(pid) and time.monotonic() < deadline:
        time.sleep(0.05)
    assert _dead(pid), (f"run={deadline - 5 - t0:.2f}s stderr={out.stderr[-300:]!r}\n"
                        + _proc_info(pid, tmp_path))


@pytest.mark.parametrize("answer,got", [("1", 1), ("sem", None)])
def test_other_protocol_means_python_alone_and_a_diary_line(
        fake_bin, tmp_path, monkeypatch, events, relog, answer, got):
    monkeypatch.setenv("FAKE_PROTOCOL", answer)
    app, port = _App(), _free_port()
    server, internal, kw = _setup(app, port)

    async def scenario():
        task = asyncio.create_task(
            rust_server.serve(server, [internal], fake_bin, kw, "tok", _bind(port)))
        await _wait_get(port, "/", lambda b: b == b"python")
        server.should_exit = True
        return await task

    assert asyncio.run(scenario()) is True
    [spawn] = _spawns(tmp_path)                     # protocolo errado não religa
    assert _dead(spawn["pid"])
    assert ("hangar_server.protocolo", "erro",
            {"esperado": rust_server.RUST_SERVER_PROTOCOL, "recebido": got}) in events
    assert ("hangar_server.reserva", "erro", {"codigo": "protocolo"}) in events


def test_run_trusts_loopback_inside_and_keeps_the_owner_list_outside(monkeypatch):
    seen = {}

    async def fake_serve(server, sockets, binary, kw, token, bind_public):
        seen["internal"] = server.config.forwarded_allow_ips
        seen["public"] = kw["forwarded_allow_ips"]
        return True

    monkeypatch.setattr(rust_server, "serve", fake_serve)
    kw = dict(host="127.0.0.1", port=1, workers=1, proxy_headers=True, forwarded_allow_ips="10.0.0.2")
    assert rust_server.run(_App(), kw, Path("x"), "tok", [], lambda: None) == 3   # nada subiu
    assert seen == {"internal": "10.0.0.2,127.0.0.1", "public": "10.0.0.2"}


def test_rust_server_off_never_looks_for_the_binary(monkeypatch):
    monkeypatch.setattr(rust_server.rust_bins, "find_bin",
                        lambda *a: pytest.fail("CP_RUST_SERVER=0 não procura binário"))
    assert rust_server.wanted_binary(False) is None


def test_missing_binary_means_python_alone_and_a_diary_line(monkeypatch, events):
    monkeypatch.setattr(rust_server.rust_bins, "find_bin", lambda name, env_var: None)
    assert rust_server.wanted_binary(True) is None
    assert ("hangar_server.reserva", "aviso", {"codigo": "sem_binario"}) in events


def test_cp_rust_server_zero_turns_it_off(monkeypatch):
    monkeypatch.setenv("CP_RUST_SERVER", "0")
    assert Settings(_env_file=None).rust_server is False
    monkeypatch.delenv("CP_RUST_SERVER")
    assert Settings(_env_file=None).rust_server is True


@pytest.mark.parametrize("ips,expected", [
    ("127.0.0.1", "127.0.0.1"),
    ("10.0.0.2", "10.0.0.2,127.0.0.1"),
    ("10.0.0.2, 127.0.0.1", "10.0.0.2, 127.0.0.1"),
    ("*", "*"),
])
def test_internal_server_trusts_the_loopback_proxy(ips, expected):
    assert rust_server.trust_loopback(ips) == expected


@pytest.mark.parametrize("host,expected", [
    ("0.0.0.0", "0.0.0.0:8765"), ("192.168.1.5", "192.168.1.5:8765"), ("::", "[::]:8765"),
])
def test_listen_addr(host, expected):
    assert rust_server.listen_addr(host, 8765) == expected


def test_hostname_bind_becomes_an_ip_literal_for_the_child():
    assert rust_server.ip_literal("localhost") in ("127.0.0.1", "::1")
    assert rust_server.ip_literal("192.168.1.5") == "192.168.1.5"
    assert rust_server.ip_literal("::") == "::"


def test_normal_shutdown_killing_the_child_is_not_a_crash(fake_bin, tmp_path, events):
    # systemctl stop / Ctrl+C mata o filho junto com o Python: sem queda no diário, sem religar.
    port, stop = _free_port(), {"now": False}
    supervisor = rust_server.Supervisor(fake_bin, "127.0.0.1", port, 1, "tok", "127.0.0.1",
                                        lambda: stop["now"])

    async def scenario():
        task = asyncio.create_task(supervisor.run())
        await _wait_get(port, rust_server.HEALTH_PATH, lambda b: json.loads(b)["ok"] is True)
        stop["now"] = True
        os.kill(supervisor.proc.pid, 9)
        return await asyncio.wait_for(task, 5)

    assert asyncio.run(scenario()) == "parada"
    assert len(_spawns(tmp_path)) == 1                 # não religou
    assert not [e for e in events if e[0] in ("hangar_server.caiu", "hangar_server.reserva")]
    assert _dead(supervisor.proc.pid)


def test_watcher_failure_puts_the_cause_in_the_diary(monkeypatch, events):
    supervisor = rust_server.Supervisor(Path("/nao-existe"), "127.0.0.1", 1, 2, "tok", "", lambda: False)

    async def boom():
        try:
            raise PermissionError(13, "negado")
        except PermissionError as e:
            raise RuntimeError("vigia") from e

    monkeypatch.setattr(supervisor, "_start", boom)
    assert asyncio.run(supervisor.run()) == "erro"
    [(evento, nivel, campos)] = [e for e in events if e[0] == "hangar_server.vigia_falhou"]
    assert nivel == "erro"
    assert campos["erro_tipo"] == "RuntimeError"
    assert (campos["causa_tipo"], campos["errno"]) == ("PermissionError", 13)
