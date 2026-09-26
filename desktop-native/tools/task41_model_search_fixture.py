"""Fixture SINTÉTICA da Task 41 (seletor de modelo: busca e lista longa): a do compositor da Task 4 com uma sessão Pi
cujo catálogo tem dezenas de modelos. Nenhuma sessão, pasta ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código de parity_composer_fixture.py sem o bloco que sobe o servidor (o arquivo dele não muda) e estende o
Handler. Tudo o que ele oferece continua valendo (sessões compose-claude, compose-codex, compose-headless).

A mais:
- sessão compose-pi (provider pi, parada);
- GET /api/sessions/compose-pi/pi/models: 60 modelos de 5 origens, o atual perto do fim, um nome bem longo;
- GET /control/t41?models=<many|short>: `short` devolve 4 modelos (a lista curta, sem busca);
- POST /api/sessions/compose-pi/pi/model: grava o escolhido como atual e o devolve.
"""
import json
import os
import pathlib
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "parity_composer_fixture.py").read_text(encoding="utf-8")
BASE = {"__name__": "parity_composer_base", "__file__": str(HERE / "parity_composer_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = ThreadingHTTPServer(")], "parity_composer_fixture.py", "exec"), BASE)

ORIGINS = ["openai", "anthropic", "google", "groq", "openrouter"]
MANY = [{"provider": ORIGINS[n % 5], "id": f"sint-{n:02d}", "name": f"Modelo sintético {n:02d}"} for n in range(1, 60)]
MANY.append({"provider": "openrouter", "id": "sint-longo",
             "name": "Modelo sintético com o nome mais longo do catálogo inteiro para conferir o corte"})
SHORT = MANY[:4]
CURRENT = {"provider": MANY[51]["provider"], "id": MANY[51]["id"]}
SHAPE = {"models": "many"}


BASE_BUILD = BASE["build"]


def build():
    sessions = BASE_BUILD()
    sessions["compose-pi"] = {"info": BASE["info"]("compose-pi", "pi"), "state": {"state": "idle"},
                              "events": [BASE["msg"]("user_msg", "p1", "Troque o modelo desta sessão.")], "suggest": ""}
    return sessions


BASE["build"] = build
BASE["SESSIONS"].clear()
BASE["SESSIONS"].update(build())


class Handler(BASE["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if url.path == "/control/t41":
            SHAPE["models"] = parse_qs(url.query).get("models", ["many"])[0]
            self.send_json(SHAPE)
            return
        if parts[:2] == ["api", "sessions"] and parts[3:] == ["pi", "models"]:
            if not self.authorized():
                return
            BASE["record"]("GET", self.path, None)
            models = SHORT if SHAPE["models"] == "short" else MANY
            self.send_json({"models": models, "current": CURRENT, "levels": ["off", "low", "medium", "high"], "thinking": "medium"})
            return
        super().do_GET()

    def do_POST(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if parts[:2] == ["api", "sessions"] and parts[3:] == ["pi", "model"]:
            if not self.authorized():
                return
            length = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(length) or b"{}")
            BASE["record"]("POST", self.path, body)
            picked = next((m for m in MANY if m["id"] == body.get("model")), None)
            if picked is None:
                self.send_json({"detail": "modelo sintético desconhecido"}, 400)
                return
            CURRENT.update(provider=picked["provider"], id=picked["id"])
            self.send_json({"current": {"name": picked["name"]}})
            return
        super().do_POST()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK41_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
