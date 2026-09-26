"""Política sintética com GET/PUT e controle de leitura, conflito, erro e queda.

GET /control/t32?read=ok|500|drop&write=ok|409|500|drop&seed=single|list|empty
  &read_delay=<seconds>&write_delay=<seconds>
GET /control/log mostra os pedidos. Nenhum PUT chega ao backend real.
"""
import json
import os
import pathlib
import time
import unicodedata
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task30_orq_fixture.py").read_text(encoding="utf-8")
BASE = {"__name__": "task30_base", "__file__": str(HERE / "task30_orq_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task30_orq_fixture.py", "exec"), BASE)

LOCK, record = BASE["LOCK"], BASE["record"]
ACCOUNTS = BASE["ACCOUNTS"]
INITIAL = BASE["POLICY"]
STATE = {"read": "ok", "write": "ok", "read_delay": 0.0, "write_delay": 0.0,
         "mtime": 1234.0, "policy": [dict(INITIAL[0])]}
LOG_DIR = pathlib.Path.home() / ".hangar/orq/2026-09-23-native-parity/visual/task32"

def account_key(value):
    return unicodedata.normalize("NFKD", value).encode("ascii", "ignore").decode().casefold().strip()


class Handler(BASE["Handler"]):
    def log_message(self, _format, *_args):
        if urlparse(self.path).path.startswith("/api/orquestracao/politica"):
            with LOG_PATH.open("a", encoding="utf-8") as log:
                log.write(f"{self.command} {urlparse(self.path).path}\n")

    def error(self, status, code, msg, **params):
        self.send_json({"detail": {"code": code, "params": params, "msg": msg}}, status)

    def server_error(self):
        body = b"Internal Server Error"
        self.send_response(500)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t32":
            query = parse_qs(url.query)
            with LOCK:
                for key in ("read", "write"):
                    if query.get(key, [""])[0] in ("ok", "409", "500", "drop"):
                        STATE[key] = query[key][0]
                    if f"{key}_delay" in query:
                        STATE[f"{key}_delay"] = min(max(float(query[f"{key}_delay"][0]), 0.0), 8.0)
                seed = query.get("seed", [""])[0]
                if seed in ("single", "list", "empty"):
                    STATE["policy"] = [dict(p) for p in INITIAL[:{"single": 1, "list": 2, "empty": 0}[seed]]]
                    STATE["mtime"] = 1234.0
                current = {k: v for k, v in STATE.items() if k != "policy"}
                current["policy"] = STATE["policy"]
            self.send_json(current)
            return
        if url.path != "/api/orquestracao/politica":
            return super().do_GET()
        record("GET", self.path, None)
        if not self.authorized():
            return
        with LOCK:
            mode, delay, mtime, policy = STATE["read"], STATE["read_delay"], STATE["mtime"], [dict(p) for p in STATE["policy"]]
        time.sleep(delay)
        if mode == "drop":
            return self.drop()
        if mode == "500":
            return self.server_error()
        self.send_json({"arquivo": "/sintetica/orquestracao-contas.md", "mtime": mtime,
                        "politica": policy, "inventario": ACCOUNTS})

    def do_PUT(self):
        path = urlparse(self.path).path
        if not path.startswith("/api/orquestracao/politica/"):
            return self.send_json({"detail": "Not Found"}, 404)
        if not self.authorized():
            return
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        record("PUT", path, body)
        account = unquote(path.rsplit("/", 1)[1])
        with LOCK:
            mode, delay = STATE["write"], STATE["write_delay"]
        time.sleep(delay)
        with LOCK:
            mtime = STATE["mtime"]
        if mode == "drop":
            return self.drop()
        if mode == "500":
            return self.server_error()
        if mode == "409" or body.get("mtime") != mtime:
            return self.error(409, "erro_orq_arquivo_mudou", "o arquivo mudou desde a leitura — recarregue")
        item = next((i for i in ACCOUNTS if i["conta"] == account and i["provider"] == body.get("provider")), None)
        if item is None:
            return self.error(400, "erro_orq_conta_desconhecida", f"conta {account!r} ({body.get('provider')}) não existe nesta máquina")
        models = [m.strip() for m in body.get("modelos", []) if m.strip()] or ["*"]
        unknown = [m for m in models if m != "*" and m not in {x["id"] for x in item["modelos"]}]
        if "*" not in models and not item["reduced"] and item["modelos"] and unknown:
            return self.error(400, "erro_orq_modelo_desconhecido", f"modelo(s) fora do catálogo da conta: {', '.join(unknown)}", modelos=unknown)
        if any("|" in v or any(ord(c) < 0x20 or c == "\x7f" for c in v) for v in (account, body.get("apelido", ""), *models)):
            return self.error(400, "erro_orq_celula_invalida", "celula com '|', quebra de linha ou caractere de controle")
        with LOCK:
            STATE["policy"] = [p for p in STATE["policy"] if p["provider"] != item["provider"] or account_key(p["conta"]) != account_key(account)]
            if body.get("ligada", True):
                STATE["policy"].append({"provider": item["provider"], "conta": account, "apelido": body.get("apelido", ""),
                                        "modelos": models, "trocar": body.get("trocar", True)})
            STATE["mtime"] += 1
            mtime = STATE["mtime"]
        self.send_json({"ok": True, "mtime": mtime})


server = BASE["BASE"]["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK32_PORT", "0"))), Handler)
LOG_DIR.mkdir(parents=True, exist_ok=True)
LOG_PATH = LOG_DIR / f"fixture-{server.server_port}.log"
LOG_PATH.write_text("fixture Task 32 iniciada\n", encoding="utf-8")
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
