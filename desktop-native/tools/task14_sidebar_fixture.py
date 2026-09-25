"""Fixture SINTÉTICA da Task 14a (barra lateral): a da Task 13 (que já carrega a de sessão e serve o build do web) com
sessões próprias e as rotas do menu da sessão. Nenhuma sessão, pasta ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 13 sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.

Rotas daqui: GET /api/push/settings, POST /api/push/mute, POST /api/sessions/<n>/rename, POST /api/sessions/<n>/open-editor,
DELETE /api/sessions/<n> e GET /api/sessions/<n>/history (a prévia pede ?limit=8).

GET /control/t14 muda como as próximas respostas saem (só as chaves dadas mudam):
  mute_read=<ok|500|404|drop>  mute=<ok|400|500|drop>  rename=<ok|409|500|drop>  editor=<ok|500|drop>
  delete=<ok|warn|500|drop>  preview=<ok|500|drop>
  <chave>_delay=<s> atrasa a rota (mute_read_delay, mute_delay, rename_delay, editor_delay, delete_delay, preview_delay).
  Renomear e fechar que dão certo mudam a lista (o SSE da lista manda a nova).
GET /control/t14reset volta sessões, silenciadas e modos ao começo.
"""
import json
import pathlib
import time
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task13_create_fixture.py").read_text(encoding="utf-8")
T13NS = {"__name__": "task13_base", "__file__": str(HERE / "task13_create_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task13_create_fixture.py", "exec"), T13NS)

BASE = T13NS["BASE"]
LOCK, SESSIONS, record, fail, bump, info, state = (BASE[k] for k in ("LOCK", "SESSIONS", "record", "fail", "bump", "info", "state"))
msg = BASE["msg"]

FIRST = {"mute_read": "ok", "mute": "ok", "rename": "ok", "editor": "ok", "delete": "ok", "preview": "ok",
         "mute_read_delay": 0.0, "mute_delay": 0.0, "rename_delay": 0.0, "editor_delay": 0.0, "delete_delay": 0.0, "preview_delay": 0.0}
T14 = dict(FIRST)
MUTED = set()

LONG = ("Terminei a troca do parser por **streaming**.\n\n- Lê o arquivo em pedaços\n- Mantém a ordem dos eventos\n- `cargo check` passou\n\n"
        "Falta rodar a suíte inteira e conferir o consumo de memória com o transcript grande (40 MB). "
        + "Linha extra para passar da altura da caixa. " * 12)

# (nome, estado, cwd, perguntas, eventos): três projetos, uma sessão sem pasta, duas aguardando, uma com "? 2".
ROWS = [
    ("sintetica-api", "idle", "/sintetica/projetos/api-sintetica", 0, [msg("assistant_msg", "a1", "Rotas novas publicadas na porta 9000.")]),
    ("sintetica-parser", "working", "/sintetica/projetos/hangar-sintetico", 0, [msg("user_msg", "p0", "Troca o parser."), msg("assistant_msg", "p1", LONG)]),
    ("Sintetica-Beta", "awaiting_input", "/sintetica/projetos/hangar-sintetico/", 2, [msg("assistant_msg", "b1", "Posso apagar a pasta `build/`?")]),
    ("sintetica-docs", "idle", "/sintetica/projetos/hangar-sintetico", 0, []),
    ("sintetica-ferramentas", "idle", "/sintetica/estudos/rust-sintetico", 0,
     [msg("assistant_msg", "f0", "Resposta antiga que não deve aparecer."), msg("user_msg", "f1", "roda o check")]
     + [{"kind": "tool_use", "id": f"f-t{i}", "text": None, "tool_name": "Bash", "tool_use_id": f"t{i}", "tool_input": {"command": "true"}}
        for i in range(8)]),
    ("sintetica-ação", "idle", "/sintetica/estudos/Área de trabalho", 0, [msg("assistant_msg", "c1", "Acentos conferidos.")]),
    ("sintetica-sem-pasta", "idle", None, 0, [msg("assistant_msg", "s1", "Sessão sem pasta.")]),
    ("sintetica-zeta", "awaiting_input", "/sintetica/estudos/rust-sintetico", 0, [msg("assistant_msg", "z1", "Qual branch uso?")]),
]


def fill():
    SESSIONS.clear()
    for name, st, cwd, questions, events in ROWS:
        data = info(name, "claude", state=st, pending_questions=questions, branch="main" if cwd else None)
        data["cwd"] = cwd
        SESSIONS[name] = {"info": data, "state": state(st, question="Posso seguir?" if st == "awaiting_input" else None),
                          "events": [dict(e) for e in events], "stats": None, "modes": []}
    bump()


fill()


class Handler(T13NS["Handler"]):
    def t14(self, key):
        """Aplica o modo da rota: atraso, queda ou erro. Devolve o modo quando a resposta ainda é desta rota."""
        with LOCK:
            mode, delay = T14[key], T14[f"{key}_delay"]
        time.sleep(delay)
        if mode == "drop":
            self.drop()
            return None
        codes = {"400": "pedido recusado (sintético)", "404": "rota sintética recusou o pedido", "409": "já existe uma sessão com esse nome (sintético)",
                 "500": "falha sintética do servidor"}
        if mode in codes:
            self.send_json(fail("erro_sintetico", codes[mode]), int(mode))
            return None
        return mode

    def do_GET(self):
        url = urlparse(self.path)
        path, query = url.path, parse_qs(url.query)
        if path == "/control/t14":
            with LOCK:
                for key, values in query.items():
                    if key in T14:
                        T14[key] = float(values[0]) if key.endswith("delay") else values[0]
                current = dict(T14)
            self.send_json(current)
            return
        if path == "/control/t14reset":
            with LOCK:
                T14.clear()
                T14.update(FIRST)
                MUTED.clear()
                fill()
            self.send_json({"ok": True})
            return
        if path.startswith("/api/") and not self.authorized():
            return
        if path == "/api/push/settings":
            record("GET", self.path, None)
            if self.t14("mute_read") is None:
                return
            with LOCK:
                self.send_json({"muted": sorted(MUTED), "quiet_hours": None})
            return
        parts = [unquote(p) for p in path.strip("/").split("/")]
        if len(parts) == 4 and parts[:2] == ["api", "sessions"] and parts[3] == "history" and parts[2] in SESSIONS:
            record("GET", self.path, None)
            if self.t14("preview") is None:
                return
            limit = int(query.get("limit", ["400"])[0])
            with LOCK:
                self.send_json(SESSIONS[parts[2]]["events"][-limit:])
            return
        super().do_GET()

    def body(self):
        length = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(length)) if length else None

    def do_POST(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        mine = url.path == "/api/push/mute" or (len(parts) == 4 and parts[:2] == ["api", "sessions"] and parts[3] in ("rename", "open-editor"))
        if not mine:
            return super().do_POST()
        body = self.body()
        record("POST", self.path, body)
        if not self.authorized():
            return
        if url.path == "/api/push/mute":
            if self.t14("mute") is None:
                return
            with LOCK:
                (MUTED.add if body.get("muted") else MUTED.discard)(body["session"])
            self.send_json({"ok": True})
            return
        name = parts[2]
        if name not in SESSIONS:
            self.send_json(fail("erro_sessao_inexistente", "sessao nao encontrada"), 404)
            return
        if parts[3] == "open-editor":
            if self.t14("editor") is not None:
                self.send_json({"ok": True})
            return
        if self.t14("rename") is None:
            return
        new = BASE_CLEAN(body.get("new", ""))
        with LOCK:
            if not new:
                self.send_json(fail("erro_nome_invalido", "nome invalido"), 400)
                return
            if new != name and new in SESSIONS:
                self.send_json(fail("erro_nome_em_uso", "ja existe uma sessao com esse nome"), 409)
                return
            s = SESSIONS.pop(name)
            s["info"]["name"] = new
            SESSIONS[new] = s
            bump()
        self.send_json({"ok": True, "name": new})

    def do_DELETE(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if not (len(parts) == 3 and parts[:2] == ["api", "sessions"]):
            return super().do_DELETE()
        record("DELETE", self.path, None)
        if not self.authorized():
            return
        mode = self.t14("delete")
        if mode is None:
            return
        with LOCK:
            if SESSIONS.pop(parts[2], None) is None:
                self.send_json(fail("erro_sessao_inexistente", "sessao nao encontrada"), 404)
                return
            bump()
        warning = {"code": "erro_pareamento_saida_falhou", "params": {}, "msg": "aviso de saída falhou: sintetica-par: fora do ar (sintético)"}
        self.send_json({"ok": True, "warning": warning if mode == "warn" else None})


BASE_CLEAN = T13NS["clean"]

server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
