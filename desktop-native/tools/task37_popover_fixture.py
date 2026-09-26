"""Fixture SINTÉTICA da Task 37 (popovers do compositor presos ao gatilho): a do compositor da Task 4 com o catálogo de
modelos do Claude e uma demora opcional nos anexos recentes. Nenhuma sessão, pasta ou conta daqui existe de verdade;
nada sai deste processo.

Carrega o código de parity_composer_fixture.py sem o bloco que sobe o servidor (o arquivo dele não muda) e estende o
Handler. Tudo o que ele oferece continua valendo (sessões compose-claude, compose-codex, compose-headless).

A mais:
- GET /api/sessions/<s>/model/options: três modelos, o do meio ativo;
- GET /control/t37?slow=<segundos>: a próxima lista de anexos recentes demora esse tempo (esqueleto na tela).
"""
import os
import pathlib
import time
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "parity_composer_fixture.py").read_text(encoding="utf-8")
BASE = {"__name__": "parity_composer_base", "__file__": str(HERE / "parity_composer_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = ThreadingHTTPServer(")], "parity_composer_fixture.py", "exec"), BASE)

MODELS = {"models": [
    {"id": "claude-haiku-4-5", "name": "Haiku 4.5", "desc": "Rápido para tarefas simples", "active": False},
    {"id": "claude-opus-5-5", "name": "Opus 5.5", "desc": "O mais capaz para trabalho complexo", "active": True},
    {"id": "claude-sonnet-5", "name": "Sonnet 5", "desc": "Equilíbrio entre custo e capacidade", "active": False},
]}
SLOW = {"next": 0.0}


class Handler(BASE["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if url.path == "/control/t37":
            SLOW["next"] = float(parse_qs(url.query).get("slow", ["0"])[0])
            self.send_json({"slow": SLOW["next"]})
            return
        if parts[:2] == ["api", "sessions"] and parts[3:] == ["model", "options"]:
            if self.authorized():
                BASE["record"]("GET", self.path, None)
                self.send_json(MODELS)
            return
        if parts[:2] == ["api", "sessions"] and parts[3:] == ["uploads"] and SLOW["next"] > 0:
            wait, SLOW["next"] = SLOW["next"], 0.0
            time.sleep(wait)
        super().do_GET()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK37_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
