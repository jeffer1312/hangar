"""Fixture isolada da Task 51; execute com o Python do backend e --directory na pasta da prova."""
import argparse
import ast
import json
import os
import struct
import threading
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit

REPO = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument("--directory", type=Path, required=True)
ROOT = parser.parse_args().directory.resolve()
ROOT.mkdir(parents=True, exist_ok=True)
os.chdir(ROOT)
for key in list(os.environ):
    if key.startswith("CP_"):
        del os.environ[key]
os.environ["CLAUDE_CONFIG_DIR"] = str(ROOT / "account")
os.environ["CP_SERVER_ID"] = "task51"
TOKEN = "fixture-task51-only"
os.environ["CP_AUTH_TOKEN"] = TOKEN
import sys
sys.path.insert(0, str(REPO / "backend"))
from app import filetree, filesearch, runtime_config
from app.config import settings, variaveis_env
from app.mensagens import erro
from app.models import ChatEvent, SessionInfo, StateEvent

CWD = ROOT / "repo"
(CWD / "src").mkdir(parents=True, exist_ok=True)
EXTERNAL = ROOT / "external" / "outside.ts"
EXTERNAL.parent.mkdir(exist_ok=True)
for path in [CWD / "src/main.rs", CWD / "src/slow.rs", EXTERNAL, CWD / "src/a b.ts", CWD / "settings.json"]:
    path.write_text("".join(f"// {path.name}: linha {n:02d}\n" for n in range(1, 91)))
(CWD / "binary.bin").write_bytes(b"\x00binary")
PHOTO = CWD / "photo.png"
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
pixels = b"".join(b"\0" + bytes([60 + y, 105, 180]) * 160 for y in range(90))
PHOTO.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 160, 90, 8, 2, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))
sessions = [SessionInfo(name="task51", cwd=str(CWD), jsonl=str(ROOT / "history.jsonl"),
    tracked=True, headless=True, state="idle", git_dirty=0).model_dump()]
state = StateEvent(session="task51", state="idle").model_dump()
body = (f"Confira `{CWD}/src/main.rs:12`, depois `main.rs:65` e [com espaço](<{CWD}/src/a b.ts:3>).\n\n"
    f"Nome puro fora do projeto: `outside.ts:24`. Citação completa: {EXTERNAL}:8.\n\n"
    f"Erro: `{CWD}/missing.rs` e `{CWD}/binary.bin`. Leitura demorada: `{CWD}/src/slow.rs:70`.\n\n"
    "Não são arquivos: `app.main`, https://example.invalid/main.ts e `.gitignore`.\n\n"
    f"```rust\n// {CWD}/src/main.rs:12 continua código\n```\n\n"
    f"Imagem já existente no visor: {PHOTO}\n\n| Item | Valor |\n| --- | --- |\n| A | 2 |\n| B | 5 |\n\nApós a tabela: `main.rs:12`.\n")
prompt = f"Confira {CWD}/src/main.rs:12 e `main.rs:12`.".ljust(390, ".")
events = [ChatEvent(kind="user_msg", id="task51-user", text=prompt, ts=1_789_999_999).model_dump(),
    ChatEvent(kind="assistant_msg", id="task51-message", text=body, ts=1_790_000_000).model_dump()]
(ROOT / "history.jsonl").write_text(json.dumps(events))

# Os corpos de configuração vêm das funções reais, com capacidades e câmbio sintéticos.
namespace = dict(runtime_config=runtime_config, settings=settings, variaveis_env=variaveis_env,
    Request=object, _painel_disponivel=lambda: False, _traducao_pensamento_disponivel=lambda: False,
    _origem_do_terminal_ok=lambda request: True, diag=SimpleNamespace(VERSAO_EM_EXECUCAO="fixture"), _usd_brl=lambda: None)
module = ast.parse((REPO / "backend/app/api.py").read_text())
for node in module.body:
    if isinstance(node, ast.FunctionDef) and node.name in {"get_config", "_somente_leitura", "cotacao_endpoint"}:
        node.decorator_list = []
        exec(compile(ast.Module(body=[node], type_ignores=[]), "api.py", "exec"), namespace)
config = namespace["get_config"](None)
stop = threading.Event()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, value, status=200, mime="application/json"):
        data = value if isinstance(value, bytes) else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self):
        if self.headers.get("Authorization") == f"Bearer {TOKEN}":
            return True
        self.reply({"detail": "Unauthorized"}, 401)
        return False

    def stream(self, event, data):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            while not stop.is_set():
                self.wfile.write(f"event: {event}\ndata: {json.dumps(data)}\n\n".encode())
                self.wfile.flush()
                if stop.wait(8):
                    break
                event, data = "ping", {}
        except (BrokenPipeError, ConnectionResetError):
            pass

    def dispatch(self):
        if not self.authorized():
            return
        route = urlsplit(self.path)
        path = parse_qs(route.query).get("path", [""])[0]
        with LOG.open("a") as log:
            log.write(f"{self.command} {route.path} {path}\n")
        if route.path == "/api/sessions":
            return self.reply(sessions)
        if route.path == "/api/sessions/events":
            return self.stream("sessions", sessions)
        if route.path.endswith("/task51/events"):
            return self.stream("state", state)
        if route.path.endswith("/history"):
            return self.reply(events)
        if route.path == "/api/config":
            return self.reply(config)
        if route.path == "/api/cotacao":
            return self.reply(namespace["cotacao_endpoint"]())
        if route.path == "/api/desktop/palette":
            return self.reply({"detail": erro("erro_sem_paleta", "sem paleta")}, 404)
        try:
            if route.path.endswith("/files/resolver") and self.command == "POST":
                paths = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))["caminhos"]
                if any(str(CWD / "src/slow.rs") == value for value in paths):
                    stop.wait(3)
                if any(value.startswith("~") or not (CWD / value).resolve().is_relative_to(ROOT) for value in paths):
                    return self.reply({"ok": {}, "faltam": paths})
                return self.reply(filesearch.resolver(str(CWD), paths))
            if route.path.endswith("/files/read"):
                return self.reply(filetree.read_file(str(CWD), path))
            if route.path.endswith("/file/text") and Path(path).resolve().is_relative_to(ROOT) and path in body:
                return self.reply(filetree.read_at(Path(path), path))
            if route.path.endswith("/file") and path == str(PHOTO):
                return self.reply(PHOTO.read_bytes(), mime="image/png")
        except (filetree.FileError, filesearch.SearchError) as exc:
            message = "Nao deu pra completar a busca." if isinstance(exc, filesearch.SearchError) else "Nao deu pra acessar esse arquivo ou pasta."
            detail = erro(exc.code, message)
            detail["params"]["msg"] = message
            return self.reply({"detail": detail}, exc.status)
        return self.reply({"detail": "Not Found"}, 404)

    do_GET = dispatch
    do_POST = dispatch


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port = server.server_port
LOG = ROOT / f"fixture-{port}.log"
config_dir = ROOT / "config" / "hangar-native"
config_dir.mkdir(parents=True, exist_ok=True)
(config_dir / "connection.json").write_text(json.dumps({"address": f"http://127.0.0.1:{port}", "token": TOKEN}))
(config_dir / "appearance.json").write_text(json.dumps({"theme": "dark", "language": "pt"}))
(ROOT / "port").write_text(str(port))
(ROOT / "pid").write_text(str(os.getpid()))
print(f"fixture port={port} pid={os.getpid()}", flush=True)
server.serve_forever()
