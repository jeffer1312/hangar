"""Fixture SINTÉTICA da Task 39 (seletor de modelo: teclado, esqueleto, erro com "Tentar novamente", vazio): a do
compositor da Task 4 com o catálogo de modelos do Claude em cinco jeitos. Nenhuma sessão, pasta ou conta daqui existe de
verdade; nada sai deste processo.

Carrega o código de parity_composer_fixture.py sem o bloco que sobe o servidor (o arquivo dele não muda) e estende o
Handler. Tudo o que ele oferece continua valendo (sessões compose-claude, compose-codex, compose-headless; o
GET /control/mode?next=slow deixa a próxima troca demorar 6 s).

A mais:
- GET /api/sessions/<s>/model/options: responde conforme o jeito escolhido;
- GET /control/t39?models=<ok|slow|error|empty|many|slowmany>: vale para as próximas leituras até mudar. `slow` demora
  4 s e devolve a lista curta; `error` responde 503; `empty` devolve lista vazia; `many` devolve 24 modelos, o ativo
  perto do fim (a lista rola até ele ao abrir); `slowmany` é o `many` depois de 4 s, com o esmaecer do painel já acabado.
"""
import os
import pathlib
import time
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "parity_composer_fixture.py").read_text(encoding="utf-8")
BASE = {"__name__": "parity_composer_base", "__file__": str(HERE / "parity_composer_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = ThreadingHTTPServer(")], "parity_composer_fixture.py", "exec"), BASE)

SHORT = [
    {"id": "claude-haiku-4-5", "name": "Haiku 4.5", "desc": "Rápido para tarefas simples", "active": False},
    {"id": "claude-opus-5-5", "name": "Opus 5.5", "desc": "O mais capaz para trabalho complexo", "active": True},
    {"id": "claude-sonnet-5", "name": "Sonnet 5", "desc": "Equilíbrio entre custo e capacidade", "active": False},
]
MANY = [{"id": f"synthetic-{n:02d}", "name": f"Modelo sintético {n:02d}", "desc": f"Origem sintética {n % 4}", "active": n == 21}
        for n in range(1, 25)]
SHAPE = {"models": "ok"}


class Handler(BASE["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if url.path == "/control/t39":
            SHAPE["models"] = parse_qs(url.query).get("models", ["ok"])[0]
            self.send_json(SHAPE)
            return
        if parts[:2] == ["api", "sessions"] and parts[3:] == ["model", "options"]:
            if not self.authorized():
                return
            BASE["record"]("GET", self.path, None)
            shape = SHAPE["models"]
            if shape in ("slow", "slowmany"):
                time.sleep(4)
            if shape == "error":
                self.send_json({"detail": {"code": "erro_sintetico", "params": {}, "msg": "catálogo indisponível"}}, 503)
                return
            models = {"empty": [], "many": MANY, "slowmany": MANY}.get(shape, SHORT)
            self.send_json({"models": models})
            return
        super().do_GET()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK39_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
