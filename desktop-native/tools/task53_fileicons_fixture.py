"""Fixture da T53: Python do backend, --directory isolado, porta aleatória."""
import argparse
import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

parser = argparse.ArgumentParser()
parser.add_argument("--directory", type=Path, required=True)
ROOT = parser.parse_args().directory.resolve()
ROOT.mkdir(parents=True, exist_ok=True)
REPO = Path(__file__).resolve().parents[2]
os.chdir(ROOT)
for key in list(os.environ):
    if key.startswith("CP_"):
        del os.environ[key]
TOKEN = "fixture-task53-only"
os.environ.update(CLAUDE_CONFIG_DIR=str(ROOT / "account"), CP_SERVER_ID="task53", CP_AUTH_TOKEN=TOKEN)
sys.path.insert(0, str(REPO / "backend"))
from app import filetree, filesearch
from app.models import ChatEvent, SessionInfo, StateEvent

CWD = ROOT / "repo"
(CWD / "src").mkdir(parents=True, exist_ok=True)
for name in ["main.rs", "client.ts", "notes.md", "favicon.ico", "unknown.xyz",
             "README.pt.md", "a-long-typescript-filename-that-must-remain-readable.ts"]:
    (CWD / "src" / name).write_text(f"// {name}\n// Conteúdo sintético da T53.\n")
(CWD / "package.json").write_text('{"name":"task53-fixture"}\n')
body = ("## Arquivos citados\n\n"
        "Rust: [main.rs](src/main.rs:2)\n\n"
        "TypeScript: [client.ts](src/client.ts:2)\n\n"
        "Markdown: [notes.md](src/notes.md)\n\n"
        "Imagem: [favicon.ico](src/favicon.ico)\n\n"
        "Pasta: [src](src)\n\n"
        "Desconhecido: [unknown.xyz](src/unknown.xyz)\n\n"
        "Nome puro: `package.json`\n\n"
        "Prefixo: [README.pt.md](src/README.pt.md)\n\n"
        "Nome longo: [arquivo](src/a-long-typescript-filename-that-must-remain-readable.ts)\n")
sessions = [SessionInfo(name="task53", cwd=str(CWD), jsonl=str(ROOT / "history.jsonl"),
                       tracked=True, headless=True, state="idle", git_dirty=0).model_dump()]
state = StateEvent(session="task53", state="idle").model_dump()
events = [ChatEvent(kind="assistant_msg", id="task53-icons", text=body, ts=1_790_000_000).model_dump()]
(ROOT / "history.jsonl").write_text(json.dumps(events))
config = {"campos": {}, "somente_leitura": {"server_id": "task53", "terminal_panel": False}, "variaveis_env": []}
stop = threading.Event()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, value, status=200):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def stream(self, event, data):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            while not stop.is_set():
                self.wfile.write(f"event: {event}\ndata: {json.dumps(data)}\n\n".encode())
                self.wfile.flush()
                stop.wait(8)
                event, data = "ping", {}
        except (BrokenPipeError, ConnectionResetError):
            pass

    def dispatch(self):
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            return self.reply({"detail": "Unauthorized"}, 401)
        route = urlsplit(self.path)
        path = parse_qs(route.query).get("path", [""])[0]
        with LOG.open("a") as log:
            log.write(f"{self.command} {route.path} {path}\n")
        if route.path == "/api/sessions":
            return self.reply(sessions)
        if route.path == "/api/sessions/events":
            return self.stream("sessions", sessions)
        if route.path == "/api/sessions/task53/events":
            return self.stream("state", state)
        if route.path.endswith("/history"):
            return self.reply(events)
        if route.path == "/api/config":
            return self.reply(config)
        if route.path == "/api/cotacao":
            return self.reply({"usd_brl": None})
        try:
            if route.path.endswith("/files/resolver") and self.command == "POST":
                paths = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))))["caminhos"]
                if any(value.startswith("~") or not (CWD / value).resolve().is_relative_to(CWD) for value in paths):
                    return self.reply({"ok": {}, "faltam": paths})
                return self.reply(filesearch.resolver(str(CWD), paths))
            if route.path.endswith("/files/read"):
                return self.reply(filetree.read_file(str(CWD), path))
        except (filetree.FileError, filesearch.SearchError) as exc:
            return self.reply({"detail": "Arquivo indisponível na fixture."}, exc.status)
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
