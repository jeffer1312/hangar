"""Fixture SINTÉTICA da Task 13 (Nova sessão): a de sessão (parity_session_fixture.py) mais as rotas da criação.

Carrega o código da fixture de sessão sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.
Nenhuma pasta, conta ou sessão daqui existe de verdade; nada sai deste processo.

GET /api/fs/roots, /api/fs/scan, /api/providers, /api/claude-configs, /api/codex-contas e
/api/sessions/creation-progress, e POST /api/sessions, respondem daqui, contra o estado em memória.

GET /control/t13 muda como as próximas respostas saem (só as chaves dadas mudam):
  roots=<ok|empty|404|500|drop>  scan=<ok|unreadable|400|403|404|500|drop>  providers=<ok|404|500|drop>
  configs=<ok|404|500|drop>  codex=<ok|404|500|drop>  sessions=<ok|404|500|drop>  (a lista que a escolha da pasta lê)
  create=<ok|rename|notes|409|400|422|500|drop>  delay=<s> (leituras)  create_delay=<s> (a criação, com os passos)
Atraso por rota: <rota>_delay=<s> (providers, configs, codex, models, context, preview...) e progress_delay (a consulta do passo).
Task 13b (escolhas finas e retomada):
  models=<ok|reduced|empty|404|500|drop>  engines=<ok|empty|500|drop>  quotas=<ok|500|drop>
  config=<ok|nokey|500|drop> (GET /api/config: a chave do Jev e o jev_padrao)  configpost=<ok|500|drop>
  context=<ok|500|drop> (GET /api/harness/codex/opcoes)  contextpost=<ok|500|drop>  context_delay=<s>
  account=<ok|missing|listfail|400|409|500|drop> (POST /api/claude-configs)  delete=<ok|listfail|409|500|drop>  account_delay=<s>
  archive=<ok|empty|500|drop> (GET /api/archive-por-cwd)  preview=<ok|empty|500|drop>  preview_delay=<s>
  resume=<ok|409|500|drop>  models_delay=<s>
  GET /control/t13reset volta contas e o jev_padrao ao começo.

A régua: fora de /api e /control, serve o build do web (T13_WEB, só leitura; padrão ~/hangar/frontend/dist) na mesma origem,
com a conexão sintética e o idioma (?lang=pt|en na primeira página) semeados no index.html que sai daqui. O service worker
não é servido. O SSE do web autentica pelo cookie, como no backend.
"""
import json
import mimetypes
import os
import pathlib
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
WEB = pathlib.Path(os.environ.get("T13_WEB", pathlib.Path.home() / "hangar/frontend/dist")).resolve()
SOURCE = (HERE / "parity_session_fixture.py").read_text(encoding="utf-8")
BASE = {"__name__": "parity_session_base", "__file__": str(HERE / "parity_session_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = ThreadingHTTPServer(")], "parity_session_fixture.py", "exec"), BASE)

LOCK, SESSIONS, record, fail, bump, info, state, TOKEN = (BASE[k] for k in ("LOCK", "SESSIONS", "record", "fail", "bump", "info", "state", "TOKEN"))

T13 = {"roots": "ok", "scan": "ok", "providers": "ok", "configs": "ok", "codex": "ok", "sessions": "ok", "create": "ok",
       "delay": 0.0, "create_delay": 0.0,
       "models": "ok", "engines": "ok", "quotas": "ok", "config": "ok", "configpost": "ok", "context": "ok", "contextpost": "ok",
       "account": "ok", "delete": "ok", "archive": "ok", "preview": "ok", "resume": "ok",
       "context_delay": 0.0, "account_delay": 0.0, "preview_delay": 0.0, "models_delay": 0.0,
       "providers_delay": 0.0, "configs_delay": 0.0, "codex_delay": 0.0, "progress_delay": 0.0}
FIRST_CONFIGS = [{"path": "/sintetica/.claude", "label": "default", "active": True},
                 {"path": "/sintetica/.claude-sintetica-trabalho", "label": "sintetica-trabalho", "active": False},
                 {"path": "/sintetica/.claude-sintetica-velha", "label": "sintetica-velha", "active": False}]
# O estado que as gravações da 13b mudam: contas Claude, o padrão do Jev e o contexto estendido do Codex.
T13B = {"configs": [dict(c) for c in FIRST_CONFIGS], "jev_padrao": False, "contexto": False}
# Passos da criação em voo, pelo nome limpo: (instante do início, passos).
CREATING = {}

ROOT_A, ROOT_B = "/sintetica/projetos", "/sintetica/estudos"
NOW = time.time()
TREE = {
    ROOT_A: [("hangar-sintetico", True, True, 600), ("api-sintetica", True, False, 7200), ("Área de trabalho", False, False, 3 * 86400),
             ("notas", False, False, 40 * 86400)],
    ROOT_A + "/hangar-sintetico": [("backend", False, True, 1200), ("frontend", False, False, 5000), ("desktop-native", False, False, 90)],
    ROOT_A + "/hangar-sintetico/backend": [],
    ROOT_B: [("rust-sintetico", True, False, 86400 * 2)],
}


def entries(path):
    return [{"name": n, "path": f"{path}/{n}", "is_git": git, "has_claude_md": md, "mtime": NOW - age}
            for n, git, md, age in TREE.get(path, [])]


def clean(name):
    """A regra de backend/app/names.py (o NFKD cobre os acentos das pastas daqui)."""
    import re
    import unicodedata
    return re.sub(r"[^A-Za-z0-9_-]", "-", unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode().strip()).strip("-")


class Handler(BASE["Handler"]):
    def authorized(self):
        if f"cp_token={TOKEN}" in (self.headers.get("Cookie") or ""):
            return True
        return super().authorized()

    def web(self, path, query):
        """O build do web, só leitura; caminho de página vira o index.html com a conexão sintética."""
        if path in ("/sw.js", "/registerSW.js"):
            self.send_json({"detail": "sem service worker na prova"}, 404)
            return
        target = (WEB / path.lstrip("/")).resolve()
        if target.is_file() and target.is_relative_to(WEB):
            data, kind = target.read_bytes(), mimetypes.guess_type(target.name)[0] or "application/octet-stream"
        else:
            lang = "en" if query.get("lang", ["pt"])[0] == "en" else "pt"
            seed = f"<script>localStorage.setItem('cp_token','{TOKEN}');localStorage.setItem('PARAGLIDE_LOCALE','{lang}')</script>"
            data, kind = (WEB / "index.html").read_text(encoding="utf-8").replace("<head>", "<head>" + seed, 1).encode(), "text/html"
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def drop(self):
        self.close_connection = True
        self.connection.shutdown(2)

    def mine(self, key, delay_key=None):
        # Atraso de uma rota só (`<rota>_delay`, ex.: providers_delay=8), além do geral.
        with LOCK:
            mode, delay = T13[key], max(T13["delay"], T13.get(delay_key or f"{key}_delay", 0.0))
        time.sleep(delay)
        if mode == "drop":
            self.drop()
            return None
        if mode in ("404", "500"):
            text = "rota sintética recusou o pedido" if mode == "404" else "falha sintética do servidor"
            self.send_json(fail("erro_sintetico", text), int(mode))
            return None
        return mode

    def do_GET(self):
        url = urlparse(self.path)
        path, query = url.path, parse_qs(url.query)
        if path == "/control/t13":
            with LOCK:
                for key, values in query.items():
                    if key in T13:
                        T13[key] = float(values[0]) if key.endswith("delay") else values[0]
                current = dict(T13)
            self.send_json(current)
            return
        if path == "/control/t13reset":
            with LOCK:
                T13B.update({"configs": [dict(c) for c in FIRST_CONFIGS], "jev_padrao": False, "contexto": False})
            self.send_json(T13B)
            return
        routes = {"/api/fs/roots": self.roots, "/api/fs/scan": self.scan, "/api/providers": self.providers,
                  "/api/claude-configs": self.configs, "/api/codex-contas": self.codex, "/api/sessions/creation-progress": self.progress,
                  "/api/model-options": self.models, "/api/engines": self.engines, "/api/cotas": self.quotas, "/api/config": self.config,
                  "/api/harness/codex/opcoes": self.context, "/api/archive-por-cwd": self.archive}
        if path.startswith("/api/archive/") and path.endswith("/history"):
            record("GET", self.path, None)
            if self.authorized():
                self.preview(path)
            return
        if path in routes:
            record("GET", self.path, None)
            if self.authorized():
                routes[path](query)
            return
        if path == "/api/sessions" and T13["sessions"] != "ok":
            record("GET", self.path, None)
            if self.authorized():
                self.mine("sessions")
            return
        if not path.startswith(("/api/", "/control/")):
            self.web(path, query)
            return
        super().do_GET()

    def roots(self, _):
        mode = self.mine("roots")
        if mode == "empty":
            self.send_json([])
        elif mode:
            self.send_json([{"name": "projetos", "path": ROOT_A}, {"name": "estudos", "path": ROOT_B}])

    def scan(self, query):
        mode = self.mine("scan")
        if not mode:
            return
        root, path = query.get("root", [""])[0], query.get("path", [""])[0] or query.get("root", [""])[0]
        codes = {"400": "path escapes its root", "403": "root not allowed", "404": "path not found"}
        if mode in codes:
            self.send_json({"detail": codes[mode]}, int(mode))
        elif root not in (ROOT_A, ROOT_B) or not path.startswith(root):
            self.send_json({"detail": "root not allowed"}, 403)
        elif mode == "unreadable":
            self.send_json({"entries": [], "error": "permission_denied"})
        else:
            self.send_json({"entries": entries(path), "error": None})

    def providers(self, _):
        if self.mine("providers"):
            self.send_json({"claude": {"disponivel": True, "motivo": None}, "codex": {"disponivel": True, "motivo": None},
                            "pi": {"disponivel": True, "motivo": None}, "kimi": {"disponivel": False, "motivo": "kimi não está no PATH"},
                            "omp": {"disponivel": True, "motivo": None}})

    def configs(self, _):
        if self.mine("configs"):
            with LOCK:
                self.send_json(list(T13B["configs"]))

    def models(self, query):
        mode = self.mine("models", "models_delay")
        if not mode:
            return
        provider = query.get("provider", ["claude"])[0]
        catalog = {
            "claude": [{"id": "default"}, {"id": "opus", "name": "Opus 5.5", "context": "1M", "vision": True},
                       {"id": "sonnet", "name": "Sonnet 5", "context": "200K"}, {"id": "haiku", "name": "Haiku 4.5"}],
            "codex": [{"id": "gpt-sintetico-sol", "name": "GPT sintético Sol", "efforts": ["low", "medium", "high", "ultra"]},
                      {"id": "gpt-sintetico-5", "name": "GPT sintético 5", "efforts": ["low", "medium"]}],
            "pi": [{"id": "k3", "provider": "kimi-sintetica", "context_length": 256000, "images": True},
                   {"id": "k3", "provider": "outra-sintetica", "context_length": 128000}],
            "kimi": [{"id": "kimi-sintetico", "name": "Kimi sintético"}],
            "omp": [{"id": "k3", "provider": "kimi-sintetica", "context_length": 256000}],
        }.get(provider, [])
        if mode == "empty":
            catalog = []
        elif mode == "reduced":
            catalog = [{"id": "opus"}, {"id": "sonnet"}, {"id": "haiku"}]
        self.send_json({"kind": provider, "reduced": mode == "reduced", "models": catalog})

    def engines(self, _):
        mode = self.mine("engines")
        if mode:
            motores = {} if mode == "empty" else {"motor-sintetico": {"label": "Motor sintético", "model": "modelo-sintetico-1"}}
            self.send_json({"motores": motores})

    def quotas(self, _):
        if not self.mine("quotas"):
            return
        now = time.time()
        self.send_json([
            {"id": "claude:/sintetica/.claude", "label": "default", "provedor": "claude", "estado": "lida",
             "janelas": [{"rotulo": "5h", "pct": 42.4, "reset_ts": now + 7800}, {"rotulo": "7d", "pct": 18.0, "reset_ts": now + 3 * 86400}]},
            {"id": "claude:/sintetica/.claude-sintetica-trabalho", "label": "sintetica-trabalho", "provedor": "claude", "estado": "lida",
             "janelas": [{"rotulo": "5h", "pct": 86.0, "reset_ts": now + 2100}, {"rotulo": "7d", "pct": 93.0, "reset_ts": now + 86400 * 1.2}]},
            {"id": "claude:/sintetica/.claude-sintetica-velha", "label": "sintetica-velha", "provedor": "claude", "estado": "expirada", "janelas": []},
            {"id": "codex:default", "label": "Padrão", "provedor": "codex", "estado": "lida", "janelas": [{"rotulo": "5h", "pct": 12.0, "reset_ts": now + 600}]},
        ])

    def config(self, _):
        mode = self.mine("config")
        if mode:
            with LOCK:
                padrao = T13B["jev_padrao"]
            self.send_json({"campos": {"jev_api_key": {"definido": mode != "nokey", "valor": "…" if mode != "nokey" else ""},
                                       "jev_padrao": {"definido": True, "valor": padrao}}})

    def context(self, _):
        if self.mine("context", "context_delay"):
            with LOCK:
                self.send_json({"contexto_estendido": T13B["contexto"], "codex_voice_beta": False})

    def archive(self, query):
        mode = self.mine("archive")
        if not mode:
            return
        cwd, provider = query.get("cwd", [""])[0], query.get("provider", ["claude"])[0]
        if mode == "empty" or provider not in ("claude", "codex"):
            self.send_json([])
            return
        now = time.time()
        entry = lambda sid, last, age, cfg, conta, live=False: {
            "project": "sintetica-projeto", "cwd": cwd, "session_id": sid, "mtime": now - age, "preview": "primeira mensagem sintética",
            "ultima": last, "live": live, "config_dir": cfg, "conta": conta, "provider": provider,
            "codex_account": "default" if provider == "codex" else None}
        self.send_json([
            entry("sintetica-viva", "conversa aberta agora (não aparece)", 30, "/sintetica/.claude", "default", live=True),
            entry("sintetica-1", "Revise o parser do relatório sintético e rode os testes", 3600, "/sintetica/.claude-sintetica-trabalho", "sintetica-trabalho"),
            entry("sintetica-2", "Ajuste a cor do botão da tela de exemplo", 86400 * 2, "/sintetica/.claude", "default"),
            entry("sintetica-3", "", 86400 * 9, "/sintetica/.claude", "default"),
        ])

    def preview(self, path):
        mode = self.mine("preview", "preview_delay")
        if not mode:
            return
        if mode == "empty":
            self.send_json([])
            return
        sid = path.split("/")[4]
        events = []
        for n in range(1, 16):
            events.append({"kind": "user_msg", "id": f"{sid}-u{n}", "text": f"Pedido sintético **{n}** da conversa `{sid}`."})
            events.append({"kind": "tool_use", "id": f"{sid}-t{n}", "tool_name": "Read", "tool_input": {"file_path": "/sintetica/x"}})
            events.append({"kind": "assistant_msg", "id": f"{sid}-a{n}", "text": f"Resposta sintética {n}:\n\n- item um\n- item dois"})
        self.send_json(events[-30:])

    def codex(self, _):
        if not self.mine("codex"):
            return
        auth = lambda status, email: {"method": "oauth", "status": status, "email": email, "plan": None}
        sync = {"status": "ready", "trust_pending": False, "issues": []}
        self.send_json([
            {"id": "default", "credential_id": "codex:default", "name": "Padrão", "home": "/sintetica/.codex", "is_default": True,
             "auth": auth("connected", "sintetica@exemplo.test"), "sync": sync},
            {"id": "sintetica-2", "credential_id": "codex:sintetica-2", "name": "sintetica-2", "home": "/sintetica/.codex-2",
             "is_default": False, "auth": auth("disconnected", None), "sync": sync},
        ])

    def progress(self, query):
        name = clean(query.get("name", [""])[0])
        with LOCK:
            slow = T13["progress_delay"]
        # Consulta lenta: o relógio do app tem de andar sozinho enquanto ela não volta.
        time.sleep(slow)
        with LOCK:
            started = CREATING.get(name)
        if started is None:
            self.send_json({"step": None, "params": {}})
            return
        elapsed = time.time() - started
        step = "preparando" if elapsed < 2 else "conta" if elapsed < 4 else "criando"
        self.send_json({"step": step, "params": {"conta": "sintetica-trabalho"} if step == "conta" else {}})

    def written(self, key, delay_key="account_delay"):
        """Modo de uma gravação da 13b: None quando a resposta já saiu (queda ou erro forçado)."""
        with LOCK:
            mode, delay = T13[key], T13.get(delay_key, 0.0)
        time.sleep(delay)
        if mode == "drop":
            self.drop()
            return None
        if mode in ("400", "409", "500"):
            text = {"400": "nome de conta inválido (sintético)", "409": "conta em uso por uma sessão viva (sintético)",
                    "500": "falha sintética do servidor"}[mode]
            self.send_json(fail("erro_sintetico", text), int(mode))
            return None
        return mode

    def do_DELETE(self):
        path = urlparse(self.path).path
        if not path.startswith("/api/claude-configs/"):
            super().do_DELETE()
            return
        record("DELETE", self.path, None)
        if not self.authorized():
            return
        name = path.rsplit("/", 1)[1]
        mode = self.written("delete")
        if not mode:
            return
        with LOCK:
            T13B["configs"] = [c for c in T13B["configs"] if c["path"] != f"/sintetica/.claude-{name}"]
            if mode == "listfail":
                T13["configs"] = "500"
        self.send_json({"ok": True})

    def do_POST(self):
        path = urlparse(self.path).path
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        if path not in ("/api/sessions", "/api/claude-configs", "/api/config", "/api/harness/codex/opcoes") \
                and not (path.startswith("/api/archive/") and path.endswith("/resume")):
            # A fixture de sessão lê o corpo de novo: devolve o que já foi lido.
            import io
            self.rfile = io.BytesIO(raw)
            super().do_POST()
            return
        body = json.loads(raw or b"{}")
        record("POST", self.path, body)
        if not self.authorized():
            return
        if path == "/api/claude-configs":
            mode = self.written("account")
            if not mode:
                return
            name = clean(body.get("nome", ""))
            created = {"path": f"/sintetica/.claude-{name}", "label": name, "active": False}
            with LOCK:
                if mode != "missing":
                    T13B["configs"].append(created)
                if mode == "listfail":
                    T13["configs"] = "500"
            self.send_json(created)
            return
        if path == "/api/config":
            if self.written("configpost", "delay"):
                with LOCK:
                    T13B["jev_padrao"] = bool(body.get("jev_padrao"))
                self.send_json({"campos": {}})
            return
        if path == "/api/harness/codex/opcoes":
            if self.written("contextpost", "context_delay"):
                with LOCK:
                    T13B["contexto"] = bool(body.get("contexto_estendido"))
                    self.send_json({"contexto_estendido": T13B["contexto"], "codex_voice_beta": False})
            return
        if path.endswith("/resume"):
            if not self.written("resume", "create_delay"):
                return
            sid = path.split("/")[4]
            name = f"retomada-{sid}"
            with LOCK:
                data = info(name, body.get("provider", "claude"))
                data["cwd"] = "/sintetica/projetos/hangar-sintetico"
                SESSIONS[name] = {"info": data, "state": state("idle"), "events": [], "stats": None}
                bump()
            self.send_json(data)
            return
        name = clean(body.get("name", ""))
        with LOCK:
            mode, delay = T13["create"], T13["create_delay"]
            CREATING[name] = time.time()
        try:
            time.sleep(delay)
            if mode == "drop":
                self.drop()
                return
            codes = {"409": fail("erro_sessao_existe", f"já existe uma sessão chamada {name}"),
                     "400": fail("erro_cwd_invalido", f"cwd invalido: {body.get('cwd')}"),
                     "422": {"detail": [{"msg": "Extra inputs are not permitted"}]},
                     "500": fail("erro_sintetico", "não consegui criar a sessão (sintético)")}
            if mode in codes:
                self.send_json(codes[mode], int(mode))
                return
            # rename: o backend devolve outro nome que o digitado (a sessão aberta tem de ser a dele).
            final = f"{name}-renomeada" if mode == "rename" else name
            with LOCK:
                if final in SESSIONS:
                    self.send_json(fail("erro_sessao_existe", f"já existe uma sessão chamada {final}"), 409)
                    return
                data = info(final, body.get("provider", "claude"), headless=bool(body.get("headless")))
                data["cwd"] = body.get("cwd")
                SESSIONS[final] = {"info": data, "state": state("idle"), "events": [], "stats": None}
                bump()
            reply = dict(data)
            if mode == "notes":
                reply["avisos"] = ["plugin sintético ligado sem instalação"]
            self.send_json(reply)
        finally:
            with LOCK:
                CREATING.pop(name, None)


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
