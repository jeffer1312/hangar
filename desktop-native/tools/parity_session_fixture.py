"""Synthetic backend for the native Task 5 checks: side panel, per-session controls and plan flows.

Nothing here talks to a real session. Every mutation is recorded (GET /control/log) and printed.
GET /control/mode?next=<ok|409|503|drop|slow> changes how the NEXT mutation answers
(`only=<route>` limits it to one route, e.g. only=input; a GET route such as only=git/files
takes next=503|409|empty).
GET /control/set?name=<s>&field=<state field>&value=<json> changes a live state field.
GET /control/reset restores the initial sessions.
GET /control/palette?status=<200|403|404>&escuro=<true|false>&delay=<s> sets what GET /api/desktop/palette answers;
each request keeps the values it saw on arrival, so a slow old answer can land after a fast new one.
GET /control/wallpaper?status=<200|403|404>&path=<image file> sets what GET /api/desktop/wallpaper answers.
GET /control/r4?rate=<n|none>&rate_status=<200|500>&rate_delay=<s>&diag=<ok|empty|404|500>&diag_delay=<s>
&update=<ok|fail|409|drop>&behind=<n>&about_delay=<s>&diag_file=<200|500> sets the Geral/Diário/Sobre routes (only the given keys change).
POST /api/atualizacao/iniciar is FAKE: it only walks a synthetic state (5 steps, a 4 s "restart" in which
GET /api/atualizacao drops the connection, then the outcome). Nothing is updated or restarted anywhere.
Contas e modelos (Task 12 R5) moram em parity_accounts_fixture.py; GET /control/r5 muda como elas respondem.
"""

import parity_accounts_fixture as accounts
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
PALETTE = {"status": 200, "escuro": True, "delay": 0.0}
WALLPAPER = {"status": 404, "path": None}
# Task 12 R4: cotação, diário de uso e atualização, todos sintéticos.
R4 = {"rate": 5.43, "rate_status": 200, "rate_delay": 0.0, "diag": "ok", "diag_delay": 0.0, "update": "ok", "behind": 3,
      "about_delay": 0.0, "offline_until": 0.0, "diag_file": 200}
UPDATE = {"backend": "2026.09.20-abc1234", "repo": "2026.09.20-abc1234",
          "estado": {"fase": "pronto", "ok": True, "texto": "Atualizado", "ts": "2026-09-20T09:00:00-03:00"}}
UPDATE_STEPS = ["Guardando o estado", "Baixando o código", "Aplicando os passos", "Instalando dependências", "Reiniciando"]
DIAG_LINES = [
    {"ts": "2026-09-24T17:42:10-03:00", "evento": "http", "nivel": "erro", "detalhe": "POST /api/sessions/hangar/input", "codigo": 503, "ms": 812,
     "tela": "chat", "sessao": "hangar"},
    {"ts": "2026-09-24T17:41:58-03:00", "evento": "sse.reconectou", "nivel": "aviso", "detalhe": "lista caiu por 4 s e voltou",
     "so": "Linux", "navegador": "Chromium 140", "vista": "desktop", "tela_px": "1536x864"},
    {"ts": "2026-09-24T17:40:02-03:00", "evento": "abriu", "detalhe": "app nativo", "tela": "config"},
    {"ts": "2026-09-23T22:15:47-03:00", "evento": "atualizacao.tique", "detalhe": "fase=rodando 3/5 " + "x" * 120, "ms": 40},
] + [{"ts": f"2026-09-23T21:{m:02d}:00-03:00", "evento": "http", "detalhe": f"GET /api/sessions/s{m}/history", "codigo": 200, "ms": m}
     for m in range(59, 43, -1)]


def update_walk(fail):
    """Estado sintético da atualização; o "reinício" derruba as leituras por 4 s."""
    for n, text in enumerate(UPDATE_STEPS, 1):
        with LOCK:
            UPDATE["estado"] = {"fase": "rodando", "passo": n, "total": len(UPDATE_STEPS), "texto": text, "ts": time.strftime("%Y-%m-%dT%H:%M:%S-03:00")}
        time.sleep(1.2)
    with LOCK:
        R4["offline_until"] = time.time() + 4
    time.sleep(4)
    with LOCK:
        now = time.strftime("%Y-%m-%dT%H:%M:%S-03:00")
        if fail:
            UPDATE["estado"] = {"fase": "pronto", "ok": False, "erro": "npm ci falhou (sintético)", "voltou": True, "ts": now}
        else:
            UPDATE["backend"] = UPDATE["repo"] = "2026.09.24-def5678"
            R4["behind"] = 0
            UPDATE["estado"] = {"fase": "pronto", "ok": True, "texto": "Atualizado", "ts": now}
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


def call(eid, tid, name, tool_input):
    return {"kind": "tool_use", "id": eid, "text": None, "tool_name": name, "tool_use_id": tid, "tool_input": tool_input}


def result(eid, tid, text, error=False):
    return {"kind": "tool_result", "id": eid, "text": None, "tool_use_id": tid, "result": text, "is_error": error}


# Conversa da Task 12 R3a: ferramentas (soltas e em grupo), pensamento com busca e comando, lista de
# tarefas incremental (TaskCreate/TaskUpdate) e uma resposta com tabela numérica.
TOOLS_EVENTS = [
    msg("user_msg", "t-u1", "Revise o parser e me mostre o custo por conta."),
    msg("thinking", "t-th1", "Preciso ver a documentação do formato antes de mexer. Depois confiro o parser atual."),
    call("t-ts", "ts", "ToolSearch", {"query": "select:WebSearch"}), result("t-ts-r", "ts", "WebSearch"),
    call("t-ws", "ws", "WebSearch", {"query": "formato jsonl claude code transcript"}), result("t-ws-r", "ws", "5 resultados\nformato jsonl"),
    call("t-grep", "gr", "Grep", {"pattern": "fn parse", "path": "src"}), result("t-grep-r", "gr", "src/parser.rs:12\nsrc/parser.rs:88"),
    msg("thinking", "t-th2", "O parser está em src/parser.rs. Vou dividir o trabalho em passos."),
    call("t-c1", "c1", "TaskCreate", {"subject": "Ler o parser atual", "description": "Entender como cada linha do jsonl vira evento.", "activeForm": "Lendo o parser atual"}),
    result("t-c1-r", "c1", "Task #1 created successfully: Ler o parser atual"),
    call("t-c2", "c2", "TaskCreate", {"subject": "Trocar a leitura por streaming", "description": "Ler o arquivo em pedaços em vez de carregar tudo.", "activeForm": "Trocando a leitura por streaming"}),
    result("t-c2-r", "c2", "Task #2 created successfully: Trocar a leitura por streaming"),
    call("t-c3", "c3", "TaskCreate", {"subject": "Rodar o check", "activeForm": "Rodando o check"}),
    result("t-c3-r", "c3", "Task #3 created successfully: Rodar o check"),
    call("t-up1", "up1", "TaskUpdate", {"taskId": "1", "status": "in_progress"}), result("t-up1-r", "up1", "Updated task #1 status"),
    call("t-r1", "r1", "Read", {"file_path": "/synthetic/hangar/src/parser.rs"}), result("t-r1-r", "r1", "\n".join(f"linha {i}" for i in range(212))),
    call("t-r2", "r2", "Read", {"file_path": "/synthetic/hangar/src/transcript.rs"}), result("t-r2-r", "r2", "\n".join(f"linha {i}" for i in range(88))),
    call("t-e1", "e1", "Edit", {"file_path": "/synthetic/hangar/src/parser.rs", "old_string": "let text = read_all(path)?;\nfor line in text.lines() {",
                                "new_string": "let file = File::open(path)?;\nlet reader = BufReader::new(file);\nfor line in reader.lines() {\n    let line = line?;"}),
    result("t-e1-r", "e1", "The file /synthetic/hangar/src/parser.rs has been updated."),
    call("t-b1", "b1", "Bash", {"command": "cargo check -p hangar-desktop", "description": "Checar o crate"}), result("t-b1-r", "b1", "Checking hangar-desktop\nFinished dev profile"),
    call("t-up2", "up2", "TaskUpdate", {"taskId": "1", "status": "completed"}), result("t-up2-r", "up2", "Updated task #1 status"),
    call("t-up3", "up3", "TaskUpdate", {"taskId": "2", "status": "in_progress"}), result("t-up3-r", "up3", "Updated task #2 status"),
    msg("assistant_msg", "t-a1", "Troquei a leitura por streaming no parser. Falta rodar o check completo."),
    call("t-b2", "b2", "Bash", {"command": "npm run check", "description": "Rodar o check do front"}),
    result("t-b2-r", "b2", "error TS2339: Property 'pendingGate' does not exist on type 'SessionState'.", True),
    msg("assistant_msg", "t-a2", "O custo por conta nesta semana:\n\n| Conta | Chamadas | Bruto |\n|---|--:|--:|\n| Kimi Code | 327 | 46,9M |\n| Claude 200 | 120 | 5,3M |\n| Codex | 48 | 1,2M |\n| OpenCode | 12 | 380k |\n\nA Kimi Code concentra a maior parte do volume."),
    # Tabela sem as bordas externas, com barra escapada no rótulo, porcentagem e milhar pt ("1.234").
    msg("assistant_msg", "t-a3", "Uso da cota por conta:\n\nConta | Uso | Custo\n--- | ---: | ---:\nKimi Code | 62% | 1.234\nCodex \\| nuvem | 25% | 980\nClaude 200 | 13% | 2.450"),
]


def long_events(rounds=60):
    """Transcript longo para a prova de rolagem: a conversa de TOOLS_EVENTS repetida, com ids e tarefas únicos por volta."""
    out = []
    for r in range(rounds):
        for ev in TOOLS_EVENTS:
            ev = dict(ev, id=f"L{r}-{ev['id']}")
            if ev.get("tool_use_id"):
                ev["tool_use_id"] = f"L{r}-{ev['tool_use_id']}"
            if ev.get("tool_name") == "TaskUpdate":
                ev["tool_input"] = dict(ev["tool_input"], taskId=str(int(ev["tool_input"]["taskId"]) + 3 * r))
            if ev["kind"] == "tool_result" and (ev.get("result") or "").startswith("Task #"):
                n = int(ev["result"].split("#")[1].split()[0]) + 3 * r
                ev["result"] = f"Task #{n} created successfully"
            out.append(ev)
    return out


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
        "p5-tools": {
            "info": info("p5-tools", "claude", git_added=5, git_removed=2, git_dirty=2, branch="main"),
            "state": state("idle", status_line="🤖 Opus5.5·1M (high✦) │ 📁 hangar [main*] │ 💬 8k/600 110k/1M │ 💵 $1.23", claude_permission_mode="manual"),
            "events": TOOLS_EVENTS, "stats": None, "modes": [], "model": "Opus 5.5",
        },
        "p5-long": {
            "info": info("p5-long", "claude", branch="main"),
            "state": state("idle", status_line="🤖 Opus5.5·1M (high✦) │ 📁 hangar [main] │ 💬 8k/600 610k/1M │ 💵 $9.80", claude_permission_mode="manual"),
            "events": long_events(), "stats": None, "modes": [], "model": "Opus 5.5",
        },
        "p5-headless": {
            "info": info("p5-headless", "claude", headless=True, git_added=0, git_removed=0, git_dirty=0, branch="feature/x"),
            "state": state("idle", status_line="🤖 Sonnet5 (medium) │ 💬 3k/400 178k/200k │ 💵 $0.81",
                           claude_permission_mode="plan", claude_previous_non_plan="acceptEdits", recarregar_motivo="config"),
            "events": [msg("user_msg", "h1", "Proponha um plano."), msg("assistant_msg", "h2", PLAN)],
            "stats": None, "modes": ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"], "model": "Sonnet 5",
        },
        "p5-codex": {
            # Duas perguntas assíncronas do Codex pendentes: a aba mostra "? 2".
            "info": info("p5-codex", "codex", branch="fix/cost", git_added=5, git_removed=2, git_dirty=1, limited=True, limit_reset="15:30",
                         pending_questions=2),
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
                PALETTE["delay"] = float(query.get("delay", ["0"])[0])
            elif path == "/control/wallpaper":
                WALLPAPER["status"] = int(query.get("status", ["200"])[0])
                WALLPAPER["path"] = query.get("path", [None])[0]
            elif path == "/control/r4":
                for key, raw in ((k, v[0]) for k, v in query.items()):
                    if key == "rate":
                        R4["rate"] = None if raw == "none" else float(raw)
                    elif key in ("rate_status", "behind", "diag_file"):
                        R4[key] = int(raw)
                    elif key in ("rate_delay", "diag_delay", "about_delay"):
                        R4[key] = float(raw)
                    elif key in ("diag", "update"):
                        R4[key] = raw
            elif path == "/control/r5":
                accounts.control(query)
            elif path == "/control/remove":
                # Sessão encerrada: some da lista ao vivo (prova do foco da aba que some).
                SESSIONS.pop(query["name"][0], None)
                bump()
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
        if path in ("/api/credenciais", "/api/engines"):
            record("GET", self.path, None)
            accounts.handle_get(self, path, query)
            return
        if path == "/api/config":
            record("GET", self.path, None)
            self.send_json({"campos": {"shortcuts": {"valor": SHORTCUTS}}})
            return
        if path == "/api/cotacao":
            record("GET", self.path, None)
            with LOCK:
                rate, status, delay = R4["rate"], R4["rate_status"], R4["rate_delay"]
            time.sleep(delay)
            if status != 200:
                self.send_json(fail("erro_sintetico", "cotação indisponível"), status)
            else:
                self.send_json({"usd_brl": rate})
            return
        if path == "/api/diag":
            record("GET", self.path, None)
            with LOCK:
                mode, delay = R4["diag"], R4["diag_delay"]
            time.sleep(delay)
            if mode in ("404", "500"):
                self.send_json({"detail": "Not Found" if mode == "404" else "diário ilegível (sintético)"}, int(mode))
            elif mode == "empty":
                self.send_json({"dias": 0, "bytes": 0, "arquivos": [], "dias_guardados": 7, "ultimas": []})
            else:
                self.send_json({"dias": 2, "bytes": 184_320, "arquivos": ["uso-2026-09-23.jsonl", "uso-2026-09-24.jsonl"],
                                "dias_guardados": 7, "ultimas": DIAG_LINES})
            return
        if path == "/api/diag/arquivo":
            record("GET", self.path, None)
            with LOCK:
                file_status = R4["diag_file"]
            if file_status != 200:
                self.send_json(fail("erro_sintetico", "não consegui ler o diário (sintético)"), file_status)
                return
            data = ("# diário SINTÉTICO da fixture parity_session_fixture\n"
                    + "".join(json.dumps(line, ensure_ascii=False) + "\n" for line in reversed(DIAG_LINES))).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/x-ndjson")
            self.send_header("Content-Disposition", 'attachment; filename="hangar-uso.jsonl"')
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        if path == "/api/atualizacao":
            record("GET", self.path, None)
            with LOCK:
                offline, delay = time.time() < R4["offline_until"], R4["about_delay"]
                behind = R4["behind"]
                body = {"versoes": {"repo": UPDATE["repo"], "backend": UPDATE["backend"]},
                        "versao_legivel": {"repo": UPDATE["repo"], "backend": UPDATE["backend"], "remoto": "2026.09.24-def5678"},
                        "atras": behind, "atualizacao_disponivel": behind > 0, "mudancas": [], "passos": [], "pre_voo": {"pode": True},
                        "estado": dict(UPDATE["estado"])}
            if offline:
                # Servidor "reiniciando": a conexão cai sem resposta.
                self.close_connection = True
                self.connection.shutdown(2)
                return
            time.sleep(delay if "procurar" not in query else max(delay, 1.5))
            self.send_json(body)
            return
        if path == "/api/desktop/palette":
            record("GET", self.path, None)
            with LOCK:
                status, dark, delay = PALETTE["status"], PALETTE["escuro"], PALETTE["delay"]
            time.sleep(delay)
            if status != 200:
                self.send_json({"detail": {"code": "erro_sem_paleta", "msg": "sem paleta"}}, status)
            else:
                self.send_json({"escuro": dark, "cores": PALETTE_DARK if dark else PALETTE_LIGHT})
            return
        if path == "/api/desktop/wallpaper":
            record("GET", self.path, None)
            with LOCK:
                status, file = WALLPAPER["status"], WALLPAPER["path"]
            if status != 200 or not file:
                self.send_json({"detail": {"code": "erro_sem_papel_de_parede", "msg": "sem papel de parede"}}, status if status != 200 else 404)
                return
            with open(file, "rb") as f:
                data = f.read()
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)
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

    def do_PUT(self):
        url = urlparse(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length)) if length else None
        record("PUT", self.path, body)
        if not self.authorized():
            return
        if not accounts.handle_put(self, url.path, body):
            self.send_json({"detail": "not found"}, 404)

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
        if url.path == "/api/atualizacao/iniciar":
            with LOCK:
                mode = R4["update"]
                running = UPDATE["estado"].get("fase") == "rodando"
            if mode == "409" or running:
                self.send_json(fail("erro_atualizacao_branch", "este checkout esta na branch mobile-expo, nao na main"), 409)
                return
            with LOCK:
                UPDATE["estado"] = {"fase": "rodando", "passo": 0, "total": len(UPDATE_STEPS), "texto": "Preparando",
                                    "ts": time.strftime("%Y-%m-%dT%H:%M:%S-03:00")}
            threading.Thread(target=update_walk, args=(mode == "fail",), daemon=True).start()
            if mode == "drop":
                # Pedido aceito, resposta perdida: o app não sabe se começou.
                self.close_connection = True
                self.connection.shutdown(2)
                return
            self.send_json({"ok": True, "pid": 0})
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
