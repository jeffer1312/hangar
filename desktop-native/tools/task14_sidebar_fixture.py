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

Task 14b1 (nenhum git de verdade roda: branches, saídas e o stash são texto sintético guardado aqui):
  GET /api/sessions/<n>/branches, POST /api/sessions/<n>/git {action: pull|stash}, POST /api/sessions/<n>/checkout {branch},
  PUT /api/sessions/<n>/then {target, text}, DELETE /api/sessions/<n>/then.
  branches=<ok|empty|409|500|drop>  pull=<ok|refused|500|404|drop>  stash=<ok|refused|500|drop>  checkout=<ok|409|500|drop>
  then=<ok|400|404|500|drop>  unlink=<ok|500|drop>, cada uma com <chave>_delay. "refused" = 200 com ok:false (o git recusou).
  Checkout e then que dão certo mudam a lista; o checkout com stash limpa a árvore suja.
Task 14b2 (bastão; nenhum resumo de verdade é montado: o texto é sintético):
  GET /api/sessions/<n>/bastao (markdown)  bastao=<ok|empty|404|500|drop>  bastao_delay=<s>
  POST /api/sessions/<n>/bastao (BastaoBody)  handoff=<ok|aviso|400|409|500|drop|lost>  handoff_delay=<s>
  "aviso" = sessão criada com o aviso de que o modelo não reescreveu; "lost" = cria a sessão e derruba a conexão sem responder.
  O passo da criação (creation-progress) anda como na Task 13 durante o atraso.
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
GIT_KEYS = ("branches", "pull", "stash", "checkout", "then", "unlink")
FIRST.update({k: "ok" for k in GIT_KEYS})
FIRST.update({f"{k}_delay": 0.0 for k in GIT_KEYS})
FIRST.update({"bastao": "ok", "bastao_delay": 0.0, "handoff": "ok", "handoff_delay": 0.0})
CREATING = T13NS["CREATING"]
DOSSIE = ("# Passagem de bastão — {n} (sintético)\n\n## Onde parou\n\nTroca do parser por **streaming** feita; falta a suíte inteira.\n\n"
          "## Arquivos mexidos\n\n- `sintetica/parser.rs`\n- `sintetica/leitor.rs`\n\n## Próximo passo\n\n1. Rodar `cargo test`\n"
          "2. Medir a memória com o transcript grande\n\n```\ngit status: 2 arquivos alterados (sintético)\n```\n"
          + "\nLinha extra para passar da altura da caixa." * 10)
T14 = dict(FIRST)
MUTED = set()
# Repositório sintético por sessão: branches (a atual é a `branch` da lista) e se a árvore está suja.
BRANCHES = ["main", "sintetica/parser-streaming", "sintetica/docs-novas", "sintetica/fix-acentos", "sintetica/experimento"]
DIRTY = set()

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
        # sintetica-docs já nasce encadeada; a ferramentas tem pasta mas não é repositório (branch nula, sem git no menu).
        data["then_target"] = "sintetica-api" if name == "sintetica-docs" else None
        if name == "sintetica-ferramentas":
            data["branch"] = None
        if data["branch"]:
            # Contagens do git na lista: é o que faz o painel da direita mostrar a seção Projeto.
            data.update(git_added=4, git_removed=1, git_dirty=2)
        SESSIONS[name] = {"info": data, "state": state(st, question="Posso seguir?" if st == "awaiting_input" else None),
                          "events": [dict(e) for e in events], "stats": None, "modes": []}
    DIRTY.clear()
    DIRTY.add("sintetica-parser")
    bump()


fill()


class Handler(T13NS["Handler"]):
    def t14(self, key, conflict=None):
        """Aplica o modo da rota: atraso, queda ou erro. Devolve o modo quando a resposta ainda é desta rota.
        `conflict` é o texto do 409 dessa rota (o git manda o stderr cru como `detail`)."""
        if conflict is not None:
            with LOCK:
                mode = T14[key]
            if mode == "409":
                time.sleep(T14[f"{key}_delay"])
                self.send_json({"detail": conflict}, 409)
                return None
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
        if len(parts) == 4 and parts[:2] == ["api", "sessions"] and parts[3] == "branches":
            record("GET", self.path, None)
            if self.t14("branches", "fatal: sintético: não é um repositório git") is None:
                return
            with LOCK:
                s = SESSIONS.get(parts[2])
                if s is None:
                    self.send_json(fail("erro_sessao_inexistente", "sessao nao encontrada"), 404)
                    return
                empty = T14["branches"] == "empty"
                current = s["info"].get("branch")
                self.send_json({"current": None if empty else current, "branches": [] if empty else BRANCHES, "remotes": [],
                                "dirty": parts[2] in DIRTY})
            return
        if len(parts) == 4 and parts[:2] == ["api", "sessions"] and parts[3] == "bastao":
            record("GET", self.path, None)
            mode = self.t14("bastao")
            if mode is None:
                return
            with LOCK:
                if parts[2] not in SESSIONS:
                    self.send_json(fail("erro_sessao_inexistente", "sessao nao encontrada"), 404)
                    return
            data = ("" if mode == "empty" else DOSSIE.format(n=parts[2])).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/markdown; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
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
        mine = url.path == "/api/push/mute" or (len(parts) == 4 and parts[:2] == ["api", "sessions"]
                                                and parts[3] in ("rename", "open-editor", "git", "checkout", "bastao"))
        if not mine:
            return super().do_POST()
        body = self.body()
        record("POST", self.path, body)
        if not self.authorized():
            return
        if parts[-1] == "bastao":
            self.handoff(parts[2], body or {})
            return
        if parts[-1] in ("git", "checkout"):
            self.git(parts[2], parts[3], body or {})
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

    def handoff(self, origin, body):
        """A passagem sintética: com o atraso, o passo da criação anda; a sessão nova nasce na lista."""
        name = BASE_CLEAN(body.get("name", ""))
        with LOCK:
            mode, delay = T14["handoff"], T14["handoff_delay"]
            CREATING[name] = time.time()
        try:
            time.sleep(delay)
            if mode == "drop":
                self.drop()
                return
            codes = {"400": fail("erro_sintetico", "provider inválido (sintético)"),
                     "409": fail("erro_sessao_existe", f"já existe uma sessão chamada {name}"),
                     "500": fail("erro_sintetico", "não consegui gravar o resumo (sintético)")}
            if mode in codes:
                self.send_json(codes[mode], int(mode))
                return
            with LOCK:
                source = SESSIONS.get(origin)
                if source is None:
                    self.send_json(fail("erro_sessao_inexistente", "sessao nao encontrada"), 404)
                    return
                if name in SESSIONS:
                    self.send_json(fail("erro_sessao_existe", f"já existe uma sessão chamada {name}"), 409)
                    return
                data = info(name, body.get("provider", "claude"), headless=bool(body.get("headless")))
                data["cwd"] = body.get("cwd") or source["info"].get("cwd")
                SESSIONS[name] = {"info": data, "state": state("idle"), "events": [], "stats": None, "modes": []}
                bump()
            if mode == "lost":
                self.drop()
                return
            aviso = "cota da sessão de origem no fim (sintético)" if mode == "aviso" else None
            self.send_json({"name": name, "dossie": f"/sintetica/.hangar/bastao/{name}.md", "texto": DOSSIE.format(n=origin),
                            "kickoff": f"[hangar: passagem de bastão] de {origin} (sintético)", "aviso": aviso})
        finally:
            with LOCK:
                CREATING.pop(name, None)

    def git(self, name, route, body):
        """Pull, stash e checkout sintéticos: só mudam o que esta fixture guarda."""
        if name not in SESSIONS:
            self.send_json(fail("erro_sessao_inexistente", "sessao nao encontrada"), 404)
            return
        if route == "checkout":
            branch = body.get("branch", "")
            if self.t14("checkout", "error: Your local changes to the following files would be overwritten by checkout:\n"
                                    "\tsintetica/arquivo.rs\nPlease commit your changes or stash them before you switch branches.\nAborting") is None:
                return
            with LOCK:
                if branch not in BRANCHES:
                    self.send_json({"detail": "branch inexistente"}, 400)
                    return
                SESSIONS[name]["info"]["branch"] = branch
                bump()
            self.send_json({"current": branch, "output": f"Switched to branch '{branch}'"})
            return
        action = body.get("action")
        if action not in ("pull", "stash"):
            self.send_json({"detail": "acao invalida"}, 400)
            return
        mode = self.t14(action)
        if mode is None:
            return
        if mode == "refused":
            out = ("fatal: Not possible to fast-forward, aborting. (sintético)" if action == "pull"
                   else "error: sintético: não consegui guardar as mudanças")
            self.send_json({"ok": False, "output": out})
            return
        if action == "stash":
            with LOCK:
                DIRTY.discard(name)
            self.send_json({"ok": True, "output": "Saved working directory and index state WIP on main: 0000000 sintético"})
            return
        self.send_json({"ok": True, "output": "Updating 0000000..1111111\nFast-forward\n sintetica/arquivo.rs | 2 +-"})

    def do_PUT(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if not (len(parts) == 4 and parts[:2] == ["api", "sessions"] and parts[3] == "then"):
            return super().do_PUT()
        body = self.body() or {}
        record("PUT", self.path, body)
        if not self.authorized():
            return
        if self.t14("then") is None:
            return
        name, target = parts[2], body.get("target", "")
        with LOCK:
            if not target or not (body.get("text") or "").strip():
                self.send_json({"detail": [{"msg": "String should have at least 1 character"}]}, 422)
            elif target == name:
                self.send_json(fail("erro_encadeamento_proprio", "sessão não pode encadear pra si mesma"), 400)
            elif name not in SESSIONS or target not in SESSIONS:
                self.send_json(fail("erro_sessao_inexistente", "sessao nao encontrada"), 404)
            else:
                SESSIONS[name]["info"]["then_target"] = target
                bump()
                self.send_json({"ok": True})

    def do_DELETE(self):
        url = urlparse(self.path)
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        if len(parts) == 4 and parts[:2] == ["api", "sessions"] and parts[3] == "then":
            record("DELETE", self.path, None)
            if not self.authorized() or self.t14("unlink") is None:
                return
            with LOCK:
                if parts[2] in SESSIONS:
                    SESSIONS[parts[2]]["info"]["then_target"] = None
                    bump()
            self.send_json({"ok": True})
            return
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
