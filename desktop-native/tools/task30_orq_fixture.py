"""Contas sintéticas para a página Orquestração (GET apenas).

GET /control/t30?state=<list|unrestricted|empty|error|drop>&delay=<seconds>
altera a próxima leitura; /control/log mostra os pedidos registrados pela base.
"""
import os
import pathlib
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task14_sidebar_fixture.py").read_text(encoding="utf-8")
BASE_NS = {"__name__": "task14_base", "__file__": str(HERE / "task14_sidebar_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task14_sidebar_fixture.py", "exec"), BASE_NS)

BASE = BASE_NS["BASE"]
LOCK, record = BASE["LOCK"], BASE["record"]
MODE = {"state": "list", "delay": 0.0}

ACCOUNTS = [
    {"provider": "claude", "conta": "sintetica-padrao", "apelido": "Claude Sintético", "id_cota": None,
     "modelos": [{"id": "opus"}, {"id": "sonnet"}], "reduced": True},
    {"provider": "codex", "conta": "sintetica-codex", "apelido": "Codex Sintético", "id_cota": None,
     "modelos": [], "reduced": False},
    {"provider": "pi", "conta": "sintetica-pi", "apelido": "Pi Sintético", "id_cota": None,
     "modelos": [{"id": "pi-model"}], "reduced": False},
    {"provider": "kimi", "conta": "sintetica-kimi", "apelido": "Kimi Sintético", "id_cota": None,
     "modelos": [{"id": "kimi-model"}], "reduced": False},
]
POLICY = [
    {"provider": "claude", "conta": "SINTÉTICA-PADRAO", "apelido": "Claude Sintético", "modelos": ["*"], "trocar": True},
    {"provider": "codex", "conta": "sintetica-codex", "apelido": "Codex Sintético", "modelos": ["gpt-6-sol"], "trocar": False},
]


class Handler(BASE_NS["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t30":
            query = parse_qs(url.query)
            with LOCK:
                state = query.get("state", [MODE["state"]])[0]
                if state in ("list", "unrestricted", "empty", "error", "drop"):
                    MODE["state"] = state
                if "delay" in query:
                    MODE["delay"] = min(max(float(query["delay"][0]), 0.0), 8.0)
                current = dict(MODE)
            self.send_json(current)
            return
        if url.path != "/api/orquestracao/politica":
            return super().do_GET()
        record("GET", self.path, None)
        if not self.authorized():
            return
        with LOCK:
            state, delay = MODE["state"], MODE["delay"]
        time.sleep(delay)
        if state == "drop":
            self.close_connection = True
        elif state == "error":
            body = b"Internal Server Error"
            self.send_response(500)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_json({"arquivo": "/sintetica/orquestracao-contas.md", "mtime": 1234.0,
                "politica": POLICY if state == "list" else [],
                "inventario": [] if state == "empty" else ACCOUNTS})


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK30_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
