"""Synthetic backend for the native Task 5 checks: side panel, per-session controls and plan flows.

Nothing here talks to a real session. Every mutation is recorded (GET /control/log) and printed.
GET /control/mode?next=<ok|409|503|drop|slow> changes how the NEXT mutation answers
(`only=<route>` limits it to one route, e.g. only=input; a GET route such as only=git/files
takes next=503|409|empty).
GET /control/set?name=<s>&field=<state field>&value=<json> changes a live state field.
GET /control/reset restores the initial sessions.
GET /control/palette?status=<200|403|404>&escuro=<true|false> sets what GET /api/desktop/palette answers.
"""

import json
import os
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

TOKEN = "parity-session-fixture"
LOCK = threading.Lock()
LOG = []
MODE = {"next": "ok", "only": None}
VERSION = {"n": 0}
# Paleta Material You sintética, no formato de backend/app/desktop_palette.py.
PALETTE = {"status": 200, "escuro": True}
PALETTE_DARK = {"background": "#15121b", "surface": "#15121b", "surfaceContainerLow": "#1d1a24", "surfaceContainer": "#221e28",
                "surfaceContainerHigh": "#2c2833", "onSurface": "#e8e0ec", "onSurfaceVariant": "#cbc3d1", "outline": "#958e9b",
                "outlineVariant": "#4a4550", "primary": "#d4bbff", "onPrimary": "#3b255f"}
PALETTE_LIGHT = {"background": "#fef7ff", "surface": "#fef7ff", "surfaceContainerLow": "#f8f1fa", "surfaceContainer": "#f2ebf4",
                 "surfaceContainerHigh": "#ece6ee", "onSurface": "#1e1a22", "onSurfaceVariant": "#4a4550", "outline": "#7b7581",
                 "outlineVariant": "#cbc3d1", "primary": "#6b4ea0", "onPrimary": "#ffffff"}
CLAUDE_CYCLE = ["manual", "acceptEdits", "plan", "auto"]
KIMI_MODELS = [("k3", "apikey", "K3", 1000000, ["low", "high"]), ("k3-256k", "apikey", "K3-256k", 262144, ["high"]),
               ("kimi-for-coding", "kimi-code", "K2.7 Coding", 262144, ["high"]),
               ("kimi-for-coding-highspeed", "kimi-code", "K2.7 Coding Highspeed", 262144, ["high"])]

SHORTCUTS = json.dumps([
    {"id": "rel", "type": "send_text", "label": "Status", "text": "Resuma o estado atual em 3 linhas."},
    {"id": "pre", "type": "send_text", "label": "Revisar", "text": "/revisar ", "send_direct": False},
    {"id": "build", "type": "shell", "label": "Build", "command": "echo sintetico", "confirm": True},
    {"id": "anexos", "type": "internal", "action": "anexos"},
    {"id": "term", "type": "internal", "action": "terminal"},
])


def msg(kind, eid, text, **extra):
    return {"kind": kind, "id": eid, "text": text, **extra}


def info(name, provider, headless=False, state="idle", jsonl=True, **extra):
    return {"name": name, "cwd": f"/synthetic/{name}", "jsonl": f"/synthetic/{name}.jsonl" if jsonl else None,
            "provider": provider, "headless": headless, "state": state, "tracked": True,
            "last_activity": time.time() - 420, **extra}


def state(value="idle", **extra):
    base = {"state": value, "label": None, "question": None, "options": None, "status_line": None, "codex_mode": None,
            "claude_permission_mode": None, "claude_previous_non_plan": None, "claude_plan_pending": None,
            "recarregar_motivo": None, "limited": False, "limit_reset": None, "loop_status": None, "loop_iter": None,
            "loop_max": None, "problema": None, "problema_detalhe": None, "login": False}
    base.update(extra)
    return base


PLAN = "# Plano sintético\n\n1. Ler o módulo.\n2. Trocar a função.\n3. Rodar o check."


def build():
    return {
        "p5-claude": {
            "info": info("p5-claude", "claude", git_added=42, git_removed=7, git_dirty=3, branch="main"),
            "state": state("idle", status_line="🤖 Opus5.5·1M (high✦) │ 📁 hangar [main*] │ 💬 12k/900 720k/1M │ ⚡5h:46% ↺34m 📅7d:57% ↺sab 18h │ 💵 $3.42 │ ⏱ 1h12m",
                           claude_permission_mode="manual"),
            "events": [msg("user_msg", "u1", "Planeje a troca do parser."), msg("assistant_msg", "a1", "Escrevi o plano no arquivo.")],
            "stats": {"turns": 4, "steps": 19, "in_tok": 812000, "out_tok": 9400, "llm_ms": 48200, "tool_ms": 12900, "tok_s": 61, "cache_pct": 93, "ttft_ms": 2100},
            "modes": [], "model": "Opus 5.5",
        },
        "p5-headless": {
            "info": info("p5-headless", "claude", headless=True, git_added=0, git_removed=0, git_dirty=0, branch="feature/x"),
            "state": state("idle", status_line="🤖 Sonnet5 (medium) │ 💬 3k/400 178k/200k │ 💵 $0.81",
                           claude_permission_mode="plan", claude_previous_non_plan="acceptEdits", recarregar_motivo="config"),
            "events": [msg("user_msg", "h1", "Proponha um plano."), msg("assistant_msg", "h2", PLAN)],
            "stats": None, "modes": ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"], "model": "Sonnet 5",
        },
        "p5-codex": {
            "info": info("p5-codex", "codex", branch="fix/cost", git_added=5, git_removed=2, git_dirty=1, limited=True, limit_reset="15:30"),
            "state": state("idle", status_line="🤖 gpt-6-astra (high) │ 💬 ctx 41k/400k │ ⚡5h:98% ↺12m", codex_mode="default", limited=True, limit_reset="15:30"),
            "events": [msg("user_msg", "c1", "Quanto custou?"), msg("assistant_msg", "c2", "Veja o painel.")],
            "stats": {"turns": 1, "steps": 1, "in_tok": 41000, "out_tok": 700}, "permission": "Full Access",
            "codex": {"model": "gpt-6-astra", "effort": "high", "mode": "default"},
        },
        "p5-pre": {
            "info": info("p5-pre", "codex", jsonl=False, state="awaiting_input", question="Aprovar os hooks deste projeto?",
                         options=["Sim, aprovar", "Não, sair"]),
            "state": state("awaiting_input"), "events": [], "stats": None,
        },
        "p5-pi": {
            "info": info("p5-pi", "pi"),
            "state": state("idle", status_line="🤖 cline-pass/kimi-k3 (high) │ 💬 sessão 251kin/10kout ctx 97k/1M │ ⚡5h:9% 📅7d:4% 🗓30d:2% │ 💵 $1.29"),
            "events": [msg("assistant_msg", "p1", "Pronto.")], "stats": None,
            "pi": {"provider": "cline-pass", "id": "kimi-k3", "thinking": "high"},
        },
        "p5-kimi": {
            "info": info("p5-kimi", "kimi", loop_status="running", loop_iter=3, loop_max=10),
            "state": state("working", label="Rodando check", status_line="🤖 K3 (high✦) │ 📁 hangar [main] │ 💬 ctx 480k/1M",
                           loop_status="running", loop_iter=3, loop_max=10),
            "events": [msg("assistant_msg", "k1", "Iteração 3.")], "stats": None, "kimi": {"model": "K3", "effort": "high"},
        },
    }


SESSIONS = build()


def bump():
    VERSION["n"] += 1


def later(seconds, change):
    def run():
        time.sleep(seconds)
        with LOCK:
            change()
            bump()
    threading.Thread(target=run, daemon=True).start()


def record(method, path, body):
    entry = {"t": round(time.time(), 3), "method": method, "path": path, "body": body}
    LOG.append(entry)
    print("REQ", json.dumps(entry, ensure_ascii=False), flush=True)


def fail(code, text):
    return {"detail": {"code": code, "params": {}, "msg": text}}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def send_json(self, body, status=200):
        data = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def frame(self, event, data, eid=None):
        chunk = f"event: {event}\n" + (f"id: {eid}\n" if eid else "") + f"data: {json.dumps(data, ensure_ascii=False)}\n\n"
        self.wfile.write(chunk.encode())
        self.wfile.flush()

    def authorized(self):
        if self.headers.get("Authorization") == f"Bearer {TOKEN}":
            return True
        self.send_json({"detail": "token"}, 401)
        return False

    def control(self, path, query):
        with LOCK:
            if path == "/control/mode":
                MODE["next"] = query.get("next", ["ok"])[0]
                MODE["only"] = query.get("only", [None])[0]
            elif path == "/control/set":
                s = SESSIONS[query["name"][0]]
                field, value = query["field"][0], json.loads(query["value"][0])
                s["state"][field] = value
                if field in ("state", "question", "options", "loop_status", "limited", "limit_reset", "label"):
                    s["info"][field] = value
                bump()
            elif path == "/control/palette":
                PALETTE["status"] = int(query.get("status", ["200"])[0])
                PALETTE["escuro"] = query.get("escuro", ["true"])[0] != "false"
            elif path == "/control/reset":
                SESSIONS.clear()
                SESSIONS.update(build())
                LOG.clear()
                bump()
            self.send_json({"mode": MODE, "log": LOG if path == "/control/log" else len(LOG)})

    def do_GET(self):
        url = urlparse(self.path)
        path, query = url.path, parse_qs(url.query)
        if path.startswith("/control/"):
            self.control(path, query)
            return
        if not self.authorized():
            return
        if path == "/api/config":
            record("GET", self.path, None)
            self.send_json({"campos": {"shortcuts": {"valor": SHORTCUTS}}})
            return
        if path == "/api/desktop/palette":
            record("GET", self.path, None)
            with LOCK:
                status, dark = PALETTE["status"], PALETTE["escuro"]
            if status != 200:
                self.send_json({"detail": {"code": "erro_sem_paleta", "msg": "sem paleta"}}, status)
            else:
                self.send_json({"escuro": dark, "cores": PALETTE_DARK if dark else PALETTE_LIGHT})
            return
        parts = [unquote(p) for p in path.strip("/").split("/")]
        name = parts[2] if len(parts) > 2 else None
        action = "/".join(parts[3:])
        if path == "/api/sessions":
            with LOCK:
                self.send_json([s["info"] for s in SESSIONS.values()])
            return
        if path == "/api/sessions/events":
            self.stream_list()
            return
        s = SESSIONS.get(name)
        if s is None:
            self.send_json({"detail": "not found"}, 404)
            return
        if action in ("history", "events") and not s["info"]["jsonl"]:
            self.send_json(fail("erro_sem_transcript", "sessão sem transcript"), 404)
            return
        if action == "history":
            with LOCK:
                self.send_json(s["events"])
        elif action == "events":
            self.stream_session(name)
        elif action == "commands":
            self.send_json([{"name": "compact", "display": "/compact", "description": "Compacta", "argumentHint": "[instruções]", "source": "builtin", "destructive": False}])
        else:
            record("GET", self.path, None)
            self.read(s, action, query)

    def read(self, s, action, query):
        provider = s["info"]["provider"]
        if MODE["only"] == action and action != "cost":
            mode, MODE["next"], MODE["only"] = MODE["next"], "ok", None
            if mode == "empty":
                return self.send_json({"files": [], "models": [], "modes": []})
            if mode in ("409", "503"):
                return self.send_json(fail("erro_sintetico", "leitura recusada"), int(mode))
        if action == "cost":
            if provider != "codex":
                return self.send_json({"detail": "not found"}, 404)
            if MODE["next"] == "503" and MODE["only"] == "cost":
                MODE["next"] = "ok"
                return self.send_json(fail("erro_sintetico", "custo indisponível agora"), 503)
            return self.send_json({"cost_usd": 1.2345, "missing_models": [], "has_usage": True})
        if action == "git/files":
            return self.send_json({"files": [
                {"path": "src/status.rs", "code": "A ", "staged": False, "added": 30, "removed": 0},
                {"path": "src/app.rs", "code": " M", "staged": False, "added": 10, "removed": 6},
                {"path": "docs/nota.md", "code": " M", "staged": False, "added": 2, "removed": 1}], "sequencer": None})
        if action == "plan-preview":
            if provider != "claude" or s["info"]["headless"]:
                return self.send_json(None)
            if query.get("content", ["false"])[0] == "true":
                return self.send_json({"name": "troca-do-parser", "path": "/synthetic/plans/troca-do-parser.md", "markdown": PLAN})
            return self.send_json({"name": "troca-do-parser", "path": "/synthetic/plans/troca-do-parser.md"})
        if action == "model/options":
            # `active` do picker real envelhece: fica no Opus mesmo depois da troca.
            models = [("opus", "Opus 5.5", "Mais capaz"), ("sonnet", "Sonnet 5", "Equilíbrio"), ("haiku", "Haiku 4.5", "Rápido")]
            return self.send_json({"kind": "claude", "engine": None, "effort": "high",
                                   "models": [{"id": i, "name": n, "desc": d, "active": i == "opus"} for i, n, d in models]})
        if action == "permission-modes":
            if query.get("sondar", ["0"])[0] == "1":
                s["modes"] = list(CLAUDE_CYCLE)
                return self.send_json({"current": s["state"]["claude_permission_mode"], "modes": s["modes"], "sondavel": True,
                                       "previous_non_plan": s["state"]["claude_previous_non_plan"], "restaurado": True})
            return self.send_json({"current": s["state"]["claude_permission_mode"], "modes": s["modes"],
                                   "sondavel": not s["info"]["headless"], "previous_non_plan": s["state"]["claude_previous_non_plan"]})
        if action == "models":
            c = s["codex"]
            return self.send_json({"models": [
                {"model": "gpt-6-astra", "displayName": "GPT-6 Astra", "description": "Padrão", "defaultEffort": "medium",
                 "efforts": [{"value": v, "description": ""} for v in ("low", "medium", "high")]},
                {"model": "gpt-6-luna", "displayName": "GPT-6 Luna", "description": "Rápido", "defaultEffort": "low",
                 "efforts": [{"value": v, "description": ""} for v in ("low", "medium")]}], "current": dict(c)})
        if action == "codex-permissions":
            names = ["Ask for approval", "Approve for me", "Full Access"]
            return self.send_json({"modes": [{"numero": i + 1, "nome": n, "desc": "", "cursor": False, "atual": n == s["permission"]} for i, n in enumerate(names)],
                                   "current": s["permission"]})
        if action == "pi/models":
            p = s["pi"]
            return self.send_json({"models": [{"provider": "cline-pass", "id": "kimi-k3", "name": "Kimi K3"}, {"provider": "cline-pass", "id": "glm-5", "name": "GLM 5"}],
                                   "current": {"provider": p["provider"], "id": p["id"]}, "thinking": p["thinking"], "levels": ["off", "low", "medium", "high"]})
        if action == "kimi/models":
            # Nomes do ~/.kimi-code/config.toml real, inclusive os que contêm outro nome.
            return self.send_json({"models": [{"alias": a, "provider": pr, "id": a, "name": n, "context_length": c, "efforts": e, "default_effort": "high"}
                                              for a, pr, n, c, e in KIMI_MODELS],
                                   "default": "k3"})
        self.send_json({"detail": "not found"}, 404)

    def stream_list(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            seen = -1
            while True:
                with LOCK:
                    if VERSION["n"] != seen:
                        seen = VERSION["n"]
                        self.frame("sessions", [s["info"] for s in SESSIONS.values()])
                    else:
                        self.frame("ping", {})
                time.sleep(1)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def stream_session(self, name):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        seen, sent = -1, set()
        try:
            while True:
                with LOCK:
                    s = SESSIONS.get(name)
                    frames = [("ping", {}, None)]
                    if s is not None and VERSION["n"] != seen:
                        seen = VERSION["n"]
                        frames = [("state", {"session": name, **s["state"]}, None), ("ask_question", None, None)]
                        if s.get("stats"):
                            frames.append(("stats", s["stats"], None))
                        for event in s["events"]:
                            if event["id"].startswith("live-") and event["id"] not in sent:
                                sent.add(event["id"])
                                frames.append(("message", event, f"fixture:{event['id']}"))
                for kind, body, eid in frames:
                    self.frame(kind, body, eid)
                time.sleep(0.3)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_POST(self):
        url = urlparse(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        parts = [unquote(p) for p in url.path.strip("/").split("/")]
        action = "/".join(parts[3:])
        body = json.loads(raw) if raw else None
        record("POST", self.path, body)
        if not self.authorized():
            return
        with LOCK:
            if MODE["only"] and MODE["only"] != action:
                mode = "ok"
            else:
                mode, MODE["next"], MODE["only"] = MODE["next"], "ok", None
        if mode == "drop":
            self.close_connection = True
            self.connection.shutdown(2)
            return
        if mode == "slow":
            time.sleep(6)
        if mode in ("409", "503"):
            detail = {"409": "a sessão recusou agora", "503": "serviço indisponível"}[mode]
            self.send_json(fail("erro_sintetico", detail), int(mode))
            return
        with LOCK:
            s = SESSIONS.get(parts[2])
            if s is None:
                self.send_json({"detail": "sessão não encontrada"}, 404)
                return
            status, reply = self.apply(s, action, body)
            bump()
        self.send_json(reply, status)

    def apply(self, s, action, body):
        st = s["state"]
        if action == "input":
            later(1.0, lambda: s["events"].append(msg("user_msg", f"live-{len(LOG)}", body["text"])))
            return 200, {"ok": True, "delivered": True}
        if action == "permission-mode":
            mode = body.get("mode")
            st["claude_permission_mode"] = mode
            if mode != "plan":
                st["claude_previous_non_plan"] = mode
            return 200, {"mode": mode, "current": mode, "previous_non_plan": st["claude_previous_non_plan"]}
        if action == "model-effort":
            if body.get("model"):
                name = {"opus": "Opus 5.5", "sonnet": "Sonnet 5", "haiku": "Haiku 4.5"}[body["model"]]
                s["model"] = name
                # A linha de status alcança a troca um pouco depois, como no pane real.
                later(2.0, lambda: st.update(status_line=re.sub(r"🤖 [^(│]+", "🤖 " + name.replace(" ", "") + " ", st["status_line"], count=1)))
            if body.get("effort"):
                later(2.0, lambda: st.update(status_line=re.sub(r"\([^)]*\)", f"({body['effort']})", st["status_line"], count=1)))
            return 200, {"ok": True, "scope": "session", "result": None}
        if action == "model":
            s["codex"].update(model=body["model"], effort=body.get("effort", s["codex"]["effort"]))
            return 200, {"ok": True}
        if action == "codex/mode":
            s["codex"]["mode"] = body["mode"]
            st["codex_mode"] = body["mode"]
            return 200, dict(s["codex"])
        if action == "codex-permissions":
            s["permission"] = body["mode"]
            return 200, {"current": body["mode"]}
        if action == "pi/model":
            p = s["pi"]
            if body.get("model"):
                p.update(provider=body["provider"], id=body["model"])
            if body.get("effort"):
                p["thinking"] = body["effort"]
            names = {"kimi-k3": "Kimi K3", "glm-5": "GLM 5"}
            return 200, {"ok": True, "current": {"provider": p["provider"], "id": p["id"], "name": names[p["id"]]}, "thinking": p["thinking"], "levels": ["off", "low", "medium", "high"]}
        if action == "kimi/model":
            if body.get("model"):
                name = {a: n for a, _, n, _, _ in KIMI_MODELS}[body["model"]]
                s["kimi"]["model"] = name
                later(2.0, lambda: st.update(status_line=re.sub(r"🤖 [^(│]+", "🤖 " + name + " ", st["status_line"], count=1)))
            if body.get("effort"):
                s["kimi"]["effort"] = body["effort"]
            return 200, {"ok": True, "current": {"alias": body.get("model"), "name": s["kimi"]["model"]}, "effort": s["kimi"]["effort"], "result": None}
        if action == "git/diff":
            return 200, {"path": body["path"], "diff": f"--- a/{body['path']}\n+++ b/{body['path']}\n@@ -1,2 +1,3 @@\n linha\n-velha\n+nova\n+outra", "truncated": False}
        if action == "shortcut-shell":
            return 202, {"ok": True}
        if action == "recarregar":
            if st["state"] != "idle":
                return 409, fail("erro_sessao_trabalhando", "a sessão está trabalhando")
            st["recarregar_motivo"] = None
            return 200, {"ok": True}
        if action == "select":
            if st["state"] != "awaiting_input":
                return 409, fail("erro_sem_menu", "nenhum menu aberto")
            s["info"].update(state="working", question=None, options=None, label="Abrindo a conversa")
            st["state"] = "working"
            later(3.0, lambda: s["info"].update(jsonl=f"/synthetic/{s['info']['name']}.jsonl", state="idle", label=None))
            return 200, {"ok": True}
        return 404, {"detail": "rota sintética desconhecida"}


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PARITY_SESSION_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port} token={TOKEN}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
