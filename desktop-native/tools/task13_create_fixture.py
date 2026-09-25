"""Fixture SINTÉTICA da Task 13 (Nova sessão): a de sessão (parity_session_fixture.py) mais as rotas da criação.

Carrega o código da fixture de sessão sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.
Nenhuma pasta, conta ou sessão daqui existe de verdade; nada sai deste processo.

GET /api/fs/roots, /api/fs/scan, /api/providers, /api/claude-configs, /api/codex-contas e
/api/sessions/creation-progress, e POST /api/sessions, respondem daqui, contra o estado em memória.

GET /control/t13 muda como as próximas respostas saem (só as chaves dadas mudam):
  roots=<ok|empty|404|500|drop>  scan=<ok|unreadable|400|403|404|500|drop>  providers=<ok|404|500|drop>
  configs=<ok|404|500|drop>  codex=<ok|404|500|drop>  sessions=<ok|404|500|drop>  (a lista que a escolha da pasta lê)
  create=<ok|rename|notes|409|400|422|500|drop>  delay=<s> (leituras)  create_delay=<s> (a criação, com os passos)
/control/log (da fixture de sessão) mostra o que foi pedido.

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
       "delay": 0.0, "create_delay": 0.0}
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

    def mine(self, key):
        with LOCK:
            mode, delay = T13[key], T13["delay"]
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
        routes = {"/api/fs/roots": self.roots, "/api/fs/scan": self.scan, "/api/providers": self.providers,
                  "/api/claude-configs": self.configs, "/api/codex-contas": self.codex, "/api/sessions/creation-progress": self.progress}
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
            self.send_json([{"path": "/sintetica/.claude", "label": "default", "active": True},
                            {"path": "/sintetica/.claude-sintetica-trabalho", "label": "sintetica-trabalho", "active": False}])

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
            started = CREATING.get(name)
        if started is None:
            self.send_json({"step": None, "params": {}})
            return
        elapsed = time.time() - started
        step = "preparando" if elapsed < 2 else "conta" if elapsed < 4 else "criando"
        self.send_json({"step": step, "params": {"conta": "sintetica-trabalho"} if step == "conta" else {}})

    def do_POST(self):
        if urlparse(self.path).path != "/api/sessions":
            super().do_POST()
            return
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"{}")
        record("POST", self.path, body)
        if not self.authorized():
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
