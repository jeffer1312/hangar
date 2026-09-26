"""Opções do Claude SINTÉTICAS da Task 38: o /api/config daqui guarda só na memória; nada do servidor real é lido ou gravado.

Base: a fixture da Task 34 (harnesses sintéticos, sobre a da Task 14), carregada sem o bloco que sobe o servidor.

GET e POST /api/config respondem daqui, com `campos` só das duas chaves do Claude (e `shortcuts` vazio, para o painel).
GET /control/t38 muda as próximas respostas (só as chaves dadas mudam):
  config=<ok|old|500|drop>  save=<ok|400|500|drop>  config_delay=<s>  save_delay=<s>  reset=1
  "old": servidor que não conhece as chaves (o card avisa em vez de sumir com elas).
  "drop" na gravação: o valor muda e a conexão cai sem resposta.
  Erros com o corpo do backend: 400 `{"detail": "<campo>: esperado true/false"}` (ValueError do runtime_config) e
  500 em texto puro `Internal Server Error` (o Starlette, para qualquer outra exceção).
Cada pedido às rotas acima vai para visual/task38/fixture-<porta>.log.
"""
import json
import os
import pathlib
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task34_harness_fixture.py").read_text(encoding="utf-8")
T34 = {"__name__": "task34_base", "__file__": str(HERE / "task34_harness_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task34_harness_fixture.py", "exec"), T34)

BASE, LOCK = T34["BASE"], T34["LOCK"]
T34["LOG_DIR"] = pathlib.Path.home() / ".hangar/orq/2026-09-23-native-parity/visual/task38"
log_line = T34["log_line"]
KEYS = ("claude_statusline_update", "claude_function_hooks")


def initial():
    return {"config": "ok", "save": "ok", "config_delay": 0.0, "save_delay": 0.0,
            "values": {"claude_statusline_update": True, "claude_function_hooks": False}}


STATE = initial()


def campos():
    fields = {"shortcuts": {"valor": "", "origem": "env"}}
    if STATE["config"] != "old":
        fields.update({k: {"valor": v, "origem": "app"} for k, v in STATE["values"].items()})
    return fields


class Handler(T34["Handler"]):
    def reply(self, mode, delay, ok, field=None):
        time.sleep(delay)
        if mode == "drop":
            self.close_connection = True
            self.connection.shutdown(2)
        elif mode == "400":
            self.send_json({"detail": f"{field}: esperado true/false"}, 400)
        elif mode == "500":
            self.server_error()
        else:
            self.send_json(ok)

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t38":
            query = parse_qs(url.query)
            with LOCK:
                if query.get("config", [""])[0] in ("ok", "old", "500", "drop"):
                    STATE["config"] = query["config"][0]
                if query.get("save", [""])[0] in ("ok", "400", "500", "drop"):
                    STATE["save"] = query["save"][0]
                for key in ("config_delay", "save_delay"):
                    if key in query:
                        STATE[key] = min(max(float(query[key][0]), 0.0), 15.0)
                if query.get("reset"):
                    STATE.clear()
                    STATE.update(initial())
                view = dict(STATE)
            self.send_json(view)
            return
        if url.path != "/api/config":
            return super().do_GET()
        log_line(self.server.server_port, "GET /api/config")
        if not self.authorized():
            return
        with LOCK:
            mode, delay, fields = STATE["config"], STATE["config_delay"], campos()
        self.reply("ok" if mode == "old" else mode, delay, {"campos": fields, "somente_leitura": {}, "variaveis_env": []})

    def do_POST(self):
        if urlparse(self.path).path != "/api/config":
            return super().do_POST()
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"{}")
        log_line(self.server.server_port, f"POST /api/config {json.dumps(body, sort_keys=True)}")
        if not self.authorized():
            return
        with LOCK:
            mode, delay = STATE["save"], STATE["save_delay"]
            if mode in ("ok", "drop"):
                STATE["values"].update({k: bool(v) for k, v in body.items() if k in KEYS})
            fields = campos()
        self.reply(mode, delay, {"campos": fields, "somente_leitura": {}}, next(iter(body), None))


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK38_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
