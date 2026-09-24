"""Synthetic backend for the native chat Task 3 checks: questions, approvals, plans and queue.

Nothing here talks to a real session. Every mutation is recorded (GET /control/log) and printed.
GET /control/mode?next=<ok|409|503|drop|slow|fallback> changes how the NEXT mutation answers;
GET /control/change swaps the pending question of every session; /control/withdraw removes it.
"""

import copy
import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

TOKEN = "parity-interactions-fixture"
LOCK = threading.Lock()
LOG = []
MODE = {"next": "ok"}
HELD = {"on": False, "pending": []}

PLAN = ("# Migrar o cache\n\n1. Ler `cache.rs`.\n2. Trocar o mapa por LRU.\n\n```rust\nlet cache = Lru::new(64);\n```\n\n- Risco: invalidação.\n\n## Passos seguintes\n\n"
        + "\n".join(f"{i}. Passo sintético {i} do plano longo." for i in range(3, 21)))

ASK_CLAUDE = {"questions": [
    {"header": "Banco", "question": "Qual banco usar no teste?", "multiSelect": False, "options": [
        {"label": "Postgres", "description": "Mesmo da produção", "preview": ""},
        {"label": "SQLite", "description": "Arquivo local, mais rápido", "preview": "sqlite3 test.db\n.schema"}]},
    {"header": "Checks", "question": "Quais verificações rodar depois?", "multiSelect": True, "options": [
        {"label": "Lint", "description": "", "preview": ""},
        {"label": "Tipos", "description": "", "preview": ""},
        {"label": "Testes", "description": "Só os afetados", "preview": ""}]},
]}
ASK_CLAUDE_CHANGED = {"questions": [
    {"header": "Porta", "question": "Qual porta o serviço deve usar?", "multiSelect": False, "options": [
        {"label": "8080", "description": "", "preview": ""}, {"label": "9090", "description": "", "preview": ""}]},
]}
ASK_CODEX = {"provider": "codex", "request_id": 42, "questions": [
    {"id": "env", "header": "Ambiente", "question": "Onde aplicar a migração?", "multiSelect": False, "isOther": True, "isSecret": False,
     "options": [{"label": "dev", "description": "Cluster de desenvolvimento"}, {"label": "staging", "description": ""}]},
    {"id": "token", "header": "Token", "question": "Cole o token de deploy.", "multiSelect": False, "isOther": True, "isSecret": True, "options": []},
]}


def info(name, provider, headless=False, state="idle"):
    return {"name": name, "cwd": f"/synthetic/{name}", "jsonl": f"/synthetic/{name}.jsonl", "provider": provider,
            "headless": headless, "state": state, "tracked": True}


def msg(kind, eid, text, **extra):
    return {"kind": kind, "id": eid, "text": text, **extra}


def build():
    base = [msg("user_msg", "u0", "Prepare o ambiente de teste."), msg("assistant_msg", "a0", "Antes, preciso de algumas decisões.")]
    return {
        "ask-claude": {"info": info("ask-claude", "claude"), "state": {"state": "awaiting_input"}, "ask": ASK_CLAUDE, "events": list(base)},
        "ask-codex": {"info": info("ask-codex", "codex"), "state": {"state": "awaiting_input"}, "ask": ASK_CODEX, "events": list(base)},
        "options-perm": {"info": info("options-perm", "claude"), "ask": None, "events": list(base),
                         "state": {"state": "awaiting_input", "question": "Permitir `Bash: rm -rf build`?", "options": ["Yes", "Yes, and don't ask again for rm", "No"]}},
        "options-multi": {"info": info("options-multi", "claude"), "ask": None, "events": list(base),
                          "state": {"state": "awaiting_input", "question": "Quais pastas limpar?", "options": ["[ ] build", "[✔] dist", "[ ] .cache"]}},
        "plan-headless": {"info": info("plan-headless", "claude", headless=True), "ask": None, "events": list(base),
                          "state": {"state": "awaiting_input", "question": "Aprovar o plano?", "options": ["Aprovar plano", "Continuar planejando"],
                                    "claude_plan_pending": {"plan": PLAN, "path": "/synthetic/.claude/plans/cache.md", "tool_use_id": "toolu_plan"}}},
        "plan-codex": {"info": info("plan-codex", "codex"), "ask": None, "state": {"state": "idle"},
                       "events": base + [msg("assistant_msg", "a-plan", "Proposta:\n\n<proposed_plan>\n" + PLAN + "\n</proposed_plan>\n")]},
        "queue-codex": {"info": info("queue-codex", "codex", state="working"), "ask": None, "state": {"state": "working"},
                        "events": base + [
                            msg("user_msg", "queued-aaa", "Também rode o lint.", queued_ts=1.0, queued_delivered=True),
                            msg("user_msg", "queued-bbb", "E me avise do resultado.", queued_ts=2.0),
                            msg("user_msg", "queued-ccc", "Mensagem que o backend desistiu de entregar.", queued_ts=3.0, desistiu=True)]},
        "ask-pi": transcript_question("ask-pi", "pi", "question", {"header": "Destino", "question": "Onde salvar o relatório?",
                                                                   "options": [{"label": "docs/", "description": "Versionado"}, {"label": "/tmp", "description": ""}]}),
        "ask-omp": transcript_question("ask-omp", "omp", "ask", {"questions": [
            {"header": "Formato", "question": "Qual formato?", "options": [{"label": "Markdown"}, {"label": "HTML"}]},
            {"header": "Extra", "question": "Não deve aparecer (omp só mostra a primeira)", "options": [{"label": "x"}]}]}),
        "ask-kimi": transcript_question("ask-kimi", "kimi", "AskUserQuestion", {"questions": [
            {"header": "Alvos", "question": "Quais alvos compilar?", "multi_select": True, "options": [{"label": "linux"}, {"label": "windows"}, {"label": "macos"}]}]}),
    }


def transcript_question(name, provider, tool, tool_input):
    events = [msg("user_msg", "u0", "Gere o relatório."),
              {"kind": "tool_use", "id": f"{name}-call", "tool_use_id": f"toolu_{name}", "tool_name": tool, "tool_input": tool_input}]
    return {"info": info(name, provider), "ask": None, "tool": f"toolu_{name}", "state": {"state": "awaiting_input"}, "events": events}


SESSIONS = build()
VERSION = {"n": 0}


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

    def frame(self, kind, body, event_id=None):
        payload = f"event: {kind}\ndata: {json.dumps(body, ensure_ascii=False)}\n"
        if event_id:
            payload += f"id: {event_id}\n"
        self.wfile.write((payload + "\n").encode())
        self.wfile.flush()

    def authorized(self):
        if self.headers.get("Authorization") == f"Bearer {TOKEN}":
            return True
        self.send_json({"detail": "unauthorized"}, 401)
        return False

    def control(self, path, query):
        with LOCK:
            if path == "/control/mode":
                MODE["next"] = query.get("next", ["ok"])[0]
            elif path == "/control/change":
                for s in SESSIONS.values():
                    if s["ask"] is ASK_CLAUDE:
                        s["ask"] = ASK_CLAUDE_CHANGED
                    elif s["ask"] is ASK_CODEX:
                        s["ask"] = {**copy.deepcopy(ASK_CODEX), "request_id": 43}
                bump()
            elif path == "/control/withdraw":
                for s in SESSIONS.values():
                    if s["ask"] is not None:
                        s["ask"] = None
                        s["state"] = {"state": "idle"}
                bump()
            elif path == "/control/hold":
                HELD["on"] = True
            elif path == "/control/release":
                HELD["on"] = False
                for deliver in HELD["pending"]:
                    deliver()
                HELD["pending"].clear()
                bump()
            elif path == "/control/next-question":
                # Nova pergunta (outro tool_use_id) na sessão Kimi.
                s = SESSIONS["ask-kimi"]
                tool_id = f"toolu_next_{len(LOG)}"
                s["tool"], s["state"] = tool_id, {"state": "awaiting_input"}
                s["events"].append({"kind": "tool_use", "id": f"live-call-{tool_id}", "tool_use_id": tool_id, "tool_name": "AskUserQuestion",
                                    "tool_input": {"questions": [{"header": "Nova", "question": "Qual canal publicar?", "options": [{"label": "estável"}, {"label": "beta"}]}]}})
                bump()
            elif path == "/control/reset":
                SESSIONS.clear()
                SESSIONS.update(build())
                LOG.clear()
                bump()
            self.send_json({"mode": MODE["next"], "log": LOG if path == "/control/log" else len(LOG)})

    def do_GET(self):
        url = urlparse(self.path)
        path = url.path
        if path.startswith("/control/"):
            self.control(path, parse_qs(url.query))
            return
        if not self.authorized():
            return
        parts = path.strip("/").split("/")
        if path == "/api/sessions":
            with LOCK:
                self.send_json([s["info"] for s in SESSIONS.values()])
        elif path == "/api/sessions/events":
            self.stream_list()
        elif len(parts) == 4 and parts[3] == "history" and parts[2] in SESSIONS:
            with LOCK:
                self.send_json(SESSIONS[parts[2]]["events"])
        elif len(parts) == 4 and parts[3] == "events" and parts[2] in SESSIONS:
            self.stream_session(parts[2])
        else:
            self.send_json({"detail": "not found"}, 404)

    def stream_list(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            with LOCK:
                self.frame("sessions", [s["info"] for s in SESSIONS.values()])
            while True:
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
                    changed = s is not None and VERSION["n"] != seen
                    if changed:
                        seen = VERSION["n"]
                        frames = [("state", s["state"], None), ("ask_question", s["ask"], None)]
                        for event in s["events"]:
                            key = json.dumps(event, sort_keys=True)
                            if key not in sent:
                                sent.add(key)
                                if event["id"].startswith("live-"):
                                    frames.append(("message", event, f"fixture:{event['id']}"))
                        for gone in s.pop("confirmed", []):
                            frames.append(("queue_confirmed", {**gone, "queued_confirmed": True}, None))
                    else:
                        frames = [("ping", {}, None)]
                for kind, body, eid in frames:
                    self.frame(kind, body, eid)
                time.sleep(0.3)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def mutate(self, method):
        path = urlparse(self.path).path
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b""
        body = json.loads(raw) if raw else None
        record(method, self.path, body)
        if not self.authorized():
            return
        with LOCK:
            mode, MODE["next"] = MODE["next"], "ok"
        if mode == "drop":
            self.close_connection = True
            self.connection.shutdown(2)
            return
        if mode == "slow":
            time.sleep(6)
        if mode == "409":
            self.send_json({"detail": {"code": "erro_codex_resposta_invalida", "params": {}, "msg": "A pergunta mudou ou não aceita essas respostas. Confira as opções e tente novamente."}}, 409)
            return
        if mode == "503":
            self.send_json({"detail": {"code": "erro_codex_resposta_envio", "params": {}, "msg": "Não foi possível confirmar o envio da resposta ao Codex."}}, 503)
            return
        parts = path.strip("/").split("/")
        name, action = parts[2], "/".join(parts[3:])
        with LOCK:
            s = SESSIONS.get(name)
            if s is None:
                self.send_json({"detail": "sessão não encontrada"}, 404)
                return
            reply = self.apply(s, action, body, mode)
            bump()
        self.send_json(*reply) if isinstance(reply, tuple) else self.send_json(reply)

    def apply(self, s, action, body, mode):
        if action == "answer" and s.get("tool"):
            # Pi/omp/Kimi: o tool_result chega ~3 s depois do 200, como no picker real.
            tool_id = s.pop("tool")
            deliver = lambda: s["events"].append({"kind": "tool_result", "id": f"live-result-{tool_id}", "tool_use_id": tool_id,
                                                  "result": json.dumps(body["answers"], ensure_ascii=False)}) or s.update(state={"state": "working"})
            # /control/hold retém o resultado até /control/release (prova do intervalo entre o 200 e o tool_result).
            if HELD["on"]:
                HELD["pending"].append(deliver)
            else:
                later(3, deliver)
            return {"ok": True, "fallback": False}
        if action == "answer":
            ask = s["ask"]
            if ask is None or ask.get("request_id") != (body or {}).get("request_id") \
                    or type(ask.get("request_id")) is not type((body or {}).get("request_id")) \
                    or len(body["answers"]) != len(ask["questions"]):
                return ({"detail": {"code": "erro_codex_resposta_invalida", "params": {}, "msg": "A pergunta mudou ou não aceita essas respostas."}}, 409)
            s["ask"], s["state"] = None, {"state": "working"}
            s["events"].append(msg("user_msg", f"live-answer-{len(LOG)}", "Respondendo à pergunta: " + json.dumps(body["answers"], ensure_ascii=False)))
            return {"ok": True, "fallback": mode == "fallback"}
        if action == "select":
            options = s["state"].get("options") or []
            index = body["option"] - 1
            if not 0 <= index < len(options):
                return ({"detail": "opção fora do seletor"}, 409)
            if options[index].startswith("["):
                label = options[index]
                options[index] = ("[✔]" + label[3:]) if label.startswith("[ ]") else ("[ ]" + label[3:])
            else:
                s["state"] = {"state": "working"}
            return {"ok": True}
        if action == "select/submit":
            s["state"] = {"state": "working"}
            return {"ok": True}
        if action == "interrupt":
            s["state"], s["ask"] = {"state": "idle"}, None
            return {"ok": True}
        if action == "codex/plan/implement":
            s["state"] = {"state": "working"}
            s["events"].append(msg("user_msg", f"live-implement-{len(LOG)}", "Implement the plan."))
            return {"ok": True}
        if action == "steer":
            # Como o `claim_undelivered` do backend: o que já foi entregue ao processo não é promovido.
            ids = [e["id"] for e in s["events"] if e["id"].startswith("queued-") and not e.get("desistiu") and not e.get("queued_delivered")]
            gone = [e for e in s["events"] if e["id"] in ids]
            s["events"] = [e for e in s["events"] if e["id"] not in ids]
            # queue_confirmed atrasado: a captura mostra o intervalo entre o 200 e a confirmação.
            later(4, lambda: s.update(confirmed=gone))
            return {"ok": True, "promoted": False, "confirmed": len(ids), "queued_ids": ids}
        if action.startswith("queue/"):
            entry = "queued-" + unquote(action.split("/", 1)[1])
            found = [e for e in s["events"] if e["id"] == entry and e.get("desistiu")]
            if not found:
                return ({"detail": {"code": "erro_fila_entrada_nao_encontrada", "params": {}, "msg": "entrada não está na fila"}}, 404)
            s["events"] = [e for e in s["events"] if e["id"] != entry]
            return {"ok": True}
        return ({"detail": "rota sintética desconhecida"}, 404)

    def do_POST(self):
        self.mutate("POST")

    def do_DELETE(self):
        self.mutate("DELETE")


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PARITY_INTERACTIONS_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port} token={TOKEN}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
