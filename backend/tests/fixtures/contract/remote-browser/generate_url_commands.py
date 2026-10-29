"""Grava os contratos executando a consulta e o cliente de comandos do Python original."""

from __future__ import annotations

import argparse
import ast
import base64
import hashlib
import hmac
import io
import json
import logging
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
from types import SimpleNamespace
from urllib.parse import quote

SOURCE = "ba2db1021ab7fb801eca50a50f9509dcb825ac7e"
OWNER = "contract-owner-token"
SECRET = "contract-internal-signature"
HERE = Path(__file__).resolve().parent
REPO = HERE.parents[4]


def clean_environment(root: Path) -> dict[str, str]:
    keep = ("PATH", "SYSTEMROOT", "WINDIR", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "TMPDIR")
    env = {key: os.environ[key] for key in keep if key in os.environ}
    home = root / "home"
    env.update({
        "HOME": str(home), "USERPROFILE": str(home),
        "HOMEDRIVE": os.path.splitdrive(str(home))[0],
        "HOMEPATH": os.path.splitdrive(str(home))[1],
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "CLAUDE_CONFIG_DIR": str(home / ".claude"),
        "CODEX_HOME": str(home / ".codex"),
        "CP_AUTH_TOKEN": OWNER, "CP_LAN_BIND_IP": "127.0.0.1",
        "CP_SCAN_ROOTS": str(root / "workspace"),
        "CP_PROJECTS_DIR": str(root / "workspace"),
        "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
        "PYTHONNOUSERSITE": "1", "PYTHONDONTWRITEBYTECODE": "1",
        "TZ": "UTC", "LC_ALL": "C", "LANG": "C",
        "NO_PROXY": "127.0.0.1,localhost,::1", "no_proxy": "127.0.0.1,localhost,::1",
    })
    return env


def capture(root: Path) -> dict:
    from fastapi import Depends, FastAPI
    from fastapi.testclient import TestClient

    sys.path.insert(0, str(root / "reference" / "backend"))
    from app import auth, guest_users, internal_api, navshell
    from app.guest_user_gate import GuestUserGate

    assert Path.home() == root / "home"
    assert auth.settings.auth_token == OWNER
    initial_threads = set(threading.enumerate())
    local_ports: set[int] = set()

    def forbid_external(event: str, args: tuple) -> None:
        if event != "socket.connect":
            return
        address = args[1]
        if not isinstance(address, tuple) or address[0] not in ("127.0.0.1", "::1") or address[1] not in local_ports:
            raise RuntimeError("a caracterização recusou uma conexão fora dos servidores temporários")

    sys.addaudithook(forbid_external)
    logger = logging.getLogger("app.api")
    warnings: list[dict] = []

    class Warnings(logging.Handler):
        def emit(self, record: logging.LogRecord) -> None:
            warnings.append({"logger": record.name, "level": record.levelname, "message": record.getMessage()})

    log_handler = Warnings(level=logging.WARNING)
    logger.addHandler(log_handler)
    logging.getLogger("hangar.navsock").addHandler(log_handler)
    app = FastAPI()
    app.add_middleware(GuestUserGate)
    source_path = root / "reference" / "backend" / "app" / "api.py"
    tree = ast.parse(source_path.read_text(encoding="utf-8"))
    handler = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "navegador_da_sessao")
    module = ast.Module(body=[handler], type_ignores=[])
    globals_ = {"app": app, "Depends": Depends, "require_auth": auth.require_auth,
                "Path": Path, "json": json, "_log": logger}
    exec(compile(ast.fix_missing_locations(module), str(source_path), "exec"), globals_)
    internal_api.set_secret(SECRET)
    nav = Path.home() / ".hangar" / "nav"
    cases: list[dict] = []
    substitutions = {str(root): "$ROOT"}

    def normalize(value):
        if isinstance(value, str):
            for old, new in substitutions.items():
                value = value.replace(old, new)
            return value
        if isinstance(value, dict):
            return {key: normalize(item) for key, item in value.items()}
        if isinstance(value, list):
            return [normalize(item) for item in value]
        return value

    def reset_files(files: dict[str, str | dict | list | bytes] | None) -> None:
        if nav.exists():
            shutil.rmtree(nav)
        if files is None:
            return
        nav.mkdir(parents=True)
        for name, content in files.items():
            target = nav / name
            if name.endswith("/"):
                target.mkdir()
            elif isinstance(content, bytes):
                target.write_bytes(content)
            else:
                target.write_text(content if isinstance(content, str) else json.dumps(content, ensure_ascii=False), encoding="utf-8")

    def file_state() -> dict:
        if not nav.is_dir():
            return {}
        return {p.name: {"directory": True} if p.is_dir() else {"bytes_hex": p.read_bytes().hex()}
                for p in sorted(nav.iterdir())}

    def get_case(name: str, files, *, session="session", scheme="http", headers=None,
                 query=None, client_host="127.0.0.1", count=1, guest=False, after_files=None,
                 ids=None, limits="Não importa o app completo nem o porteiro Rust; executa o handler original, autenticação e middleware Python."):
        reset_files(files)
        warnings.clear()
        auth.reset_backoff()
        target = f"/api/sessions/{quote(session, safe='')}/navegador"
        request_headers = headers if headers is not None else {"Authorization": f"Bearer {OWNER}"}
        params = dict(query or {})
        if guest:
            payload = base64.urlsafe_b64encode(json.dumps({"s": {session: "share"}, "exp": 4102444800}).encode()).rstrip(b"=")
            mac = hmac.new(SECRET.encode(), b"GET\n" + target.encode() + b"\n" + payload, hashlib.sha256).hexdigest()
            request_headers = {"x-hangar-guest": payload.decode() + "." + mac}
        before = file_state()
        responses = []
        with TestClient(app, base_url=f"{scheme}://contract.invalid:8765", client=(client_host, 1000), raise_server_exceptions=False) as client:
            for index in range(count):
                if index and after_files is not None:
                    reset_files(after_files)
                response = client.get(target, headers=request_headers, params=params)
                try:
                    body = response.json()
                except ValueError:
                    body = {"text": response.text}
                responses.append({"status": response.status_code, "body": body,
                                  "headers": dict(response.headers)})
        expected = {"responses": responses, "warnings": list(warnings), "files_before": before, "files_after": file_state()}
        inputs = {"files": encode_files(files), "session": session, "scheme": scheme,
                  "headers": request_headers, "query": params, "client_host": client_host,
                  "count": count, "signed_guest": guest, "after_files": encode_files(after_files)}
        cases.append(normalize({"name": name, "ids": ids or ["NB017", "NB019", "NB021", "NB022", "NB023", "NR001"],
                                "inputs": inputs, "expected": expected, "seam": "GET original extraído por AST com decorador intacto, FastAPI e autenticação reais", "limits": limits}))
        return expected

    native = {"chave": "server::session", "url": "https://example.invalid/página?q=ação#parte", "targetId": None, "pid": 123, "ts": 1,
              "ativa": 1, "abas": [{"id": 1, "url": "https://another.invalid/"}]}
    electron = {"chave": "session", "url": "http://127.0.0.1:3000/", "targetId": "target"}
    absent = get_case("get_directory_absent", None)
    assert absent["responses"][0]["body"] == {"url": None}
    get_case("get_empty_directory", {})
    get_case("get_native_top_level_url", {"native.json": native}, ids=["NB021", "NB023", "NB024", "NR013"])
    get_case("get_electron_exact_name", {"electron.json": electron})
    get_case("get_missing_url", {"native.json": {"chave": "server::session"}})
    get_case("get_url_null", {"native.json": {"chave": "server::session", "url": None}})
    get_case("get_url_empty", {"native.json": {"chave": "server::session", "url": ""}})
    get_case("get_other_session", {"other.json": {"chave": "server::other", "url": "https://other.invalid/"}})
    get_case("get_unicode_session", {"native.json": {"chave": "server::sessão-🚀", "url": "https://example.invalid/ação"}}, session="sessão-🚀")
    get_case("get_ignores_json_array", {"array.json": [], "native.json": native})
    get_case("get_ignores_json_null", {"null.json": "null", "native.json": native})
    get_case("get_bad_json_warns_and_continues", {"broken.json": "{", "native.json": native}, ids=["NB020"])
    get_case("get_bad_utf8_warns_and_continues", {"broken.json": b"\xff", "native.json": native}, ids=["NB020"])
    get_case("get_directory_with_json_suffix_warns", {"directory.json/": "", "native.json": native}, ids=["NB020"])
    get_case("get_auxiliary_files_do_not_match", {"_srv.json": {"porta": 1234, "token": "synthetic"}, "_pendentes.json": {"session": {"url": "https://pending.invalid/"}}, "native.json": native})
    get_case("get_underscore_file_with_matching_key", {"_matching.json": {"chave": "session", "url": "https://auxiliary.invalid/"}}, ids=["NB019", "NB021"])
    get_case("get_duplicate_suffix_creation_order_a_b", {"a.json": native, "b.json": {**native, "chave": "second::session", "url": "https://second.invalid/"}})
    get_case("get_duplicate_suffix_creation_order_b_a", {"b.json": {**native, "chave": "second::session", "url": "https://second.invalid/"}, "a.json": native})
    get_case("get_no_existence_or_live_pid_check", {"old.json": {**native, "pid": 2147483647, "ts": -1}}, ids=["NB021", "NB028", "NR035"])
    get_case("get_renamed_session_does_not_match_old_key", {"native.json": native}, session="renamed", ids=["NB021", "NR035"])
    get_case("get_recreated_name_still_reads_old_sidecar", {"native.json": native}, ids=["NB021", "NR035"])
    get_case("get_reloads_changed_file", {"native.json": native}, count=2, after_files={"native.json": {**native, "url": "https://updated.invalid/"}}, ids=["NB019"])
    get_case("get_reloads_deleted_file", {"native.json": native}, count=2, after_files={}, ids=["NB019"])
    for name, headers, query, scheme in [
        ("missing", {}, {}, "http"),
        ("wrong_bearer", {"Authorization": "Bearer wrong"}, {}, "http"),
        ("query", {}, {"token": OWNER}, "http"),
        ("cookie_http", {"Cookie": f"cp_token={OWNER}"}, {}, "http"),
        ("cookie_https_ignored", {"Cookie": f"cp_token={OWNER}"}, {}, "https"),
        ("host_cookie_https", {"Cookie": f"__Host-cp_token={OWNER}"}, {}, "https"),
        ("bearer_over_query", {"Authorization": "Bearer wrong"}, {"token": OWNER}, "http"),
        ("query_over_cookie", {"Cookie": f"cp_token={OWNER}"}, {"token": "wrong"}, "http"),
        ("empty_bearer_over_query", {"Authorization": "Bearer "}, {"token": OWNER}, "http"),
        ("empty_query_uses_cookie", {"Cookie": f"cp_token={OWNER}"}, {"token": ""}, "http"),
    ]:
        get_case("get_auth_" + name, {"native.json": native}, headers=headers, query=query, scheme=scheme, ids=["NB017", "NR001"])
    get_case("get_signed_share_reaches_handler", {"native.json": native}, guest=True, ids=["NB018", "NR005"], limits="Cabeçalho assinado real do transporte Python; a autorização por sessão e nível do porteiro Rust não é executada.")
    _, login_token = guest_users.create("contract-guest", str(root / "workspace"), False, True)
    denied = get_case("get_login_guest_is_blocked", {"native.json": native}, headers={"Authorization": f"Bearer {login_token}"}, ids=["NB018", "NR007"])
    cases[-1]["inputs"]["headers"]["Authorization"] = "Bearer $GUEST_TOKEN"
    assert denied["responses"][0]["status"] == 403
    auth_clock_epoch = 1000.0
    original_auth_time = auth.time
    try:
        auth.time = SimpleNamespace(time=lambda: auth_clock_epoch)
        limited = get_case("get_auth_rate_limit_remote", {"native.json": native}, headers={"Authorization": "Bearer wrong"}, client_host="198.51.100.9", count=9, ids=["NR003"], limits="Contador real com relógio auth.time fixo em 1000,0 segundos; uma suspensão do processo não envelhece as oito falhas.")
        cases[-1]["inputs"]["clock_epoch_seconds"] = auth_clock_epoch
        assert [response["status"] for response in limited["responses"]] == [401] * 8 + [429]
    finally:
        auth.time = original_auth_time
    get_case("get_auth_loopback_not_rate_limited", {"native.json": native}, headers={"Authorization": "Bearer wrong"}, count=9, ids=["NR003"])

    @contextmanager
    def desktop(label: str, status=200, body=b"layout: desktop", *, close=False, hold=False):
        requests = []
        release = threading.Event()

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                raw = self.rfile.read(int(self.headers["Content-Length"]))
                requests.append({"method": "POST", "path": self.path, "headers": dict(self.headers),
                                 "body_utf8": raw.decode("utf-8"), "body_json": json.loads(raw)})
                if close:
                    self.close_connection = True
                    return
                if hold:
                    assert release.wait(10), "o cenário não liberou a resposta HTTP pendente"
                    self.close_connection = True
                    return
                self.send_response(status)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        port = server.server_address[1]
        local_ports.add(port)
        substitutions[f"127.0.0.1:{port}"] = f"$SERVER_{label}"
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, name=f"contract-http-{label}")
        thread.start()
        try:
            yield port, requests
        finally:
            release.set()
            server.shutdown()
            server.server_close()
            thread.join(timeout=10)
            assert not thread.is_alive(), "servidor HTTP temporário continuou vivo"
            local_ports.remove(port)

    def command_case(name: str, files, *, verb="layout", arguments=None, tab=None, ids=None, limits="Cliente navshell original contra HTTP loopback temporário; não executa o controlador gráfico.", server_status=200, server_body=b"layout: desktop", close=False, hold=False, timeout=None):
        with desktop("A", server_status, server_body, close=close, hold=hold) as (port, requests):
            resolved = {key: ({**value, "porta": port} if key == "_srv.json" and isinstance(value, dict) and value.get("porta") == "$PORT_A" else value)
                        for key, value in (files or {}).items()} if files is not None else None
            reset_files(resolved)
            warnings.clear()
            try:
                call = ("session", verb, arguments, tab)
                result = {"value": navshell.verbo(*call, teto_s=timeout) if timeout is not None else navshell.verbo(*call)}
            except Exception as exc:
                result = {"exception": type(exc).__name__, "message": str(exc)}
            expected = {**result, "requests": requests, "warnings": list(warnings)}
            inputs = {"files": encode_files(files), "name": "session", "verb": verb, "args": arguments, "tab": tab,
                      "response_status": server_status, "response_body_hex": server_body.hex(), "close_without_response": close,
                      "hold_response_until_cleanup": hold, "timeout_seconds": timeout}
        cases.append(normalize({"name": name, "ids": ids or ["NB001", "NB003", "NB004", "NB008", "NB009", "NB011"],
                                "inputs": inputs, "expected": expected, "seam": "navshell.verbo original com servidor HTTP real", "limits": limits}))
        return expected

    server_config = {"porta": "$PORT_A", "token": "desktop-contract-token", "pid": 123}
    files = {"native.json": native, "_srv.json": server_config}
    for name, config in [("missing_directory", None), ("missing_server", {"native.json": native}),
                         ("invalid_server_json", {"_srv.json": "{"}), ("server_array", {"_srv.json": []}),
                         ("server_without_port", {"_srv.json": {"token": "synthetic"}}),
                         ("server_without_token", {"_srv.json": {"porta": 123}}),
                         ("server_without_browser", {"_srv.json": server_config})]:
        command_case("cmd_" + name, config, ids=["NB002", "NB006", "NB007"])
    normal = command_case("cmd_native_without_target", files)
    assert normal["value"] == "layout: desktop"
    assert normal["requests"][0]["body_json"] == {"chave": "server::session", "verbo": "layout", "args": []}
    command_case("cmd_electron", {"electron.json": electron, "_srv.json": server_config})
    command_case("cmd_dimensions", files, arguments=["900", "700"], ids=["NB008", "NB009", "NB014"])
    command_case("cmd_tab_zero", files, arguments=["desktop"], tab=0, ids=["NB009"])
    command_case("cmd_unicode_body", files, verb="fill", arguments=["ação 🚀", "linha\nseguinte"], tab=2, server_body="ok: ação 🚀".encode(), ids=["NB008", "NB009", "NB011"])
    command_case("cmd_invalid_utf8_response", files, server_body=b"ok: \xff\xc3", ids=["NB011"])
    command_case("cmd_http_error_body", files, server_status=409, server_body="erro: navegador pausado".encode(), ids=["NB012"])
    command_case("cmd_http_auth_error_body", files, server_status=401, server_body=b"desktop unauthorized", ids=["NB012"])
    command_case("cmd_empty_response", files, server_body=b"", ids=["NB011"])
    command_case("cmd_disconnect_without_response", files, close=True, ids=["NB013"])
    command_case("cmd_socket_timeout", files, hold=True, timeout=0.1, ids=["NB010", "NB013"],
                 limits="Resposta só é liberada na limpeza; o teto solicitado é 0,1 segundo, não uma medição de desempenho nem o padrão de 45 segundos.")
    command_case("cmd_bad_sidecar_warns_before_valid", {"broken.json": "{", **files}, ids=["NB005", "NB020"])
    command_case("cmd_ignores_underscore_matching_browser", {"_matching.json": {"chave": "session", "url": "https://ignored.invalid/"}, "_srv.json": server_config}, ids=["NB005", "NB006"])
    with socket.socket() as refused:
        refused.bind(("127.0.0.1", 0))
        port = refused.getsockname()[1]
        local_ports.add(port)
        substitutions[f"127.0.0.1:{port}"] = "$SERVER_REFUSED"
        reset_files({"native.json": native, "_srv.json": {**server_config, "porta": port}})
        try:
            try:
                outcome = {"value": navshell.verbo("session", "layout")}
            except Exception as exc:
                outcome = {"exception": type(exc).__name__, "message": str(exc)}
            cases.append(normalize({"name": "cmd_connection_refused", "ids": ["NB013"],
                                    "inputs": {"files": encode_files(files), "server_listening": False},
                                    "expected": outcome, "seam": "navshell.verbo original contra socket local reservado sem listen",
                                    "limits": "Código e texto do sistema operacional são conservados; fixture identifica a plataforma."}))
            assert outcome["exception"] == "ShellIndisponivel"
        finally:
            local_ports.remove(port)
    with desktop("A", body=b"layout: mobile") as (port_a, requests_a), desktop("B", body=b"layout: desktop") as (port_b, requests_b):
        reset_files({"native.json": native, "_srv.json": {**server_config, "porta": port_a, "token": "first-token"}})
        first = navshell.verbo("session", "layout")
        (nav / "_srv.json").write_text(json.dumps({**server_config, "porta": port_b, "token": "second-token", "pid": 456}), encoding="utf-8")
        second = navshell.verbo("session", "layout")
        cases.append(normalize({"name": "cmd_reloads_server_after_restart", "ids": ["NB001", "NB028", "NR014"],
                                "inputs": {"files": encode_files(files), "server_a_body": "layout: mobile", "server_b_body": "layout: desktop", "first_token": "first-token", "second_token": "second-token", "second_pid": 456},
                                "expected": {"values": [first, second], "requests_a": requests_a, "requests_b": requests_b},
                                "seam": "Duas chamadas reais de navshell.verbo com troca de _srv.json e dois servidores HTTP", "limits": "Não reinicia o desktop real; troca sua fronteira externa de descoberta e transporte."}))
        assert [first, second] == ["layout: mobile", "layout: desktop"]
        assert requests_a[0]["headers"]["Authorization"] == "Bearer first-token"
        assert requests_b[0]["headers"]["Authorization"] == "Bearer second-token"
    assert navshell.TETO_S == 45.0
    logger.removeHandler(log_handler)
    logging.getLogger("hangar.navsock").removeHandler(log_handler)
    remaining = [thread.name for thread in threading.enumerate() if thread not in initial_threads and thread.is_alive()]
    assert not remaining, f"threads deixadas pela caracterização: {remaining}"
    return {"source_commit": SOURCE,
            "generator": "backend/.venv/bin/python backend/tests/fixtures/contract/remote-browser/generate_url_commands.py",
            "python_version": sys.version.split()[0], "platform": os.name, "sys_platform": sys.platform,
            "clock_injection": {"case": "get_auth_rate_limit_remote", "boundary": "auth.time.time", "epoch_seconds": auth_clock_epoch},
            "seams": {"get": "Handler original completo extraído da AST, decorador intacto, auth e GuestUserGate originais; sem lifespan/app principal.",
                      "commands": "Cliente navshell original contra servidores HTTP loopback temporários."},
            "normalization": {"$ROOT": "Somente prefixo do temporário em mensagens/arquivos.", "$SERVER_A": "Host e porta efêmera do primeiro servidor HTTP.", "$SERVER_B": "Host e porta efêmera do segundo servidor HTTP.", "$SERVER_REFUSED": "Host e porta do socket reservado sem listen.", "$PORT_A": "Porta efêmera injetada no arquivo de entrada do servidor A.", "$GUEST_TOKEN": "Token aleatório criado pela API original de convidados, usado apenas como entrada de autorização."},
            "limits": ["Não executa o porteiro Rust, o app principal nem navegador gráfico.", "O teto padrão de 45 segundos foi conferido na implementação, sem aguardar esse prazo. O timeout curto de 0,1 segundo foi exercitado com a resposta HTTP retida; desconexão e conexão recusada também foram exercitadas separadamente.", "Ordem de diretório é a observada neste sistema; os casos duplicados registram a enumeração efetiva, sem ordenar os sidecars.", "Nenhuma conexão é permitida fora das portas efêmeras HTTP pertencentes a esta execução."],
            "cases": cases}


def encode_files(files):
    if files is None:
        return None
    return {name: {"bytes_hex": value.hex()} if isinstance(value, bytes) else {"directory": True} if name.endswith("/") else {"content": value}
            for name, value in files.items()}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=HERE / "url-commands.json")
    parser.add_argument("--capture", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.capture:
        print(json.dumps(capture(args.capture), ensure_ascii=False, indent=2))
        return
    with tempfile.TemporaryDirectory(prefix="hangar-browser-url-contract-") as temporary:
        root = Path(temporary)
        (root / "home").mkdir()
        (root / "workspace").mkdir()
        reference = root / "reference"
        reference.mkdir()
        archive = subprocess.run(["git", "archive", "--format=tar", SOURCE, "backend/app"], cwd=REPO,
                                 check=True, capture_output=True, timeout=30)
        with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as bundle:
            bundle.extractall(reference, filter="data")
        try:
            child = subprocess.run([sys.executable, str(Path(__file__).resolve()), "--capture", str(root)],
                                   cwd=root / "workspace", env=clean_environment(root), check=True,
                                   capture_output=True, text=True, encoding="utf-8", timeout=120)
        except subprocess.CalledProcessError as exc:
            sys.stderr.write(exc.stderr)
            raise
        payload = json.loads(child.stdout)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"Caracterização gravada: {len(payload['cases'])} casos, origem {SOURCE}")


if __name__ == "__main__":
    main()
