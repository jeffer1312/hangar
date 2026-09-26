#!/usr/bin/env python3
"""Fixture isolada: --output-dir DIR; controles POST /fixture/{settings,state,reconnect}."""
import argparse
import datetime
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit


SESSION = "task57-notifications"
TOKEN = "task57-synthetic"
condition = threading.Condition()
state, mode, delay, revision, reconnect = "idle", "normal", 0.0, 0, 0
settings_reads = 0
notify_finished, finish_min_seconds = True, 0


def nullable(names):
    return dict.fromkeys(names.split())


def session_body():
    # SessionInfo de models.py, usado por list_sessions e list_events.
    return {
        **nullable("engine codex_home conta last_reply last_reply_at branch git_dirty git_ahead git_behind "
                   "git_added git_removed label question options problema limit_reset then_target status_line "
                   "pair_peers pair_gid pair_task loop_status loop_iter loop_max plan_name plan_task "
                   "plan_task_total plan_done plan_total plan_complete plan_tasks plan_hidden"),
        "name": SESSION, "cwd": "/fixture/task57", "jsonl": "/fixture/task57/session.jsonl",
        "provider": "codex", "headless": True, "state": state, "tracked": True,
        "last_activity": 1790424000.0, "worktree": False, "avisos": [], "startup_steps": [],
        "pending_questions": 0, "stalled": False, "limited": False,
    }


def state_body():
    # StateEvent emitido por merged_events.
    return {
        **nullable("codex_mode codex_question claude_permission_mode claude_previous_non_plan "
                   "claude_plan_pending label question options status_line limit_reset loop_status "
                   "loop_iter loop_max problema problema_detalhe recarregar_motivo"),
        "session": SESSION, "state": state, "codex_buffering": False, "overlay": False,
        "login": False, "limited": False, "headless": True, "shells": [],
        "question": "Continuar a prova?" if state == "awaiting_input" else None,
    }


def preferences(selected):
    # get_push_prefs; janela cruza meia-noite e inclui o minuto atual.
    now = datetime.datetime.now()
    quiet = {"start": "22:00", "end": "21:59"} if (now.hour, now.minute) == (23, 58) else {
        "start": "23:59", "end": "23:58"}
    return {"muted": [SESSION] if selected == "muted" else [],
            "quiet_hours": quiet if selected == "quiet" else None}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def reply(self, body, status=200):
        raw = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def error_body(self, code, message, status):
        self.reply({"detail": {"code": code, "params": {}, "msg": message}}, status)

    def authorized(self):
        if self.headers.get("Authorization") == f"Bearer {TOKEN}":
            return True
        # require_auth -> mensagens.erro.
        self.error_body("erro_nao_autorizado", "unauthorized", 401)
        return False

    def stream(self, listing):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        with condition:
            generation, last = reconnect, -1
        try:
            while True:
                with condition:
                    condition.wait_for(lambda: revision != last or reconnect != generation, timeout=2)
                    if reconnect != generation:
                        break
                    changed, last = revision != last, revision
                    event = ("sessions" if listing else "state") if changed else "ping"
                    body = ([session_body()] if listing else state_body()) if changed else {}
                self.wfile.write(f"event: {event}\ndata: {json.dumps(body)}\n\n".encode())
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        self.close_connection = True

    def do_GET(self):
        global settings_reads
        path = urlsplit(self.path).path
        self.server.record(path)
        if path == "/fixture/status":
            return self.reply({"state": state, "mode": mode, "settings_reads": settings_reads})
        if not self.authorized():
            return
        if path == "/api/push/settings":
            with condition:
                selected, seconds = mode, delay
                settings_reads += 1
            time.sleep(seconds)
            if selected == "error":
                return self.error_body("erro_nao_autorizado", "unauthorized", 401)
            return self.reply(preferences(selected))
        if path == "/api/sessions":
            return self.reply([session_body()])
        if path in ("/api/sessions/events", f"/api/sessions/{SESSION}/events"):
            return self.stream(path == "/api/sessions/events")
        if path == f"/api/sessions/{SESSION}/history":
            return self.reply([])  # merged_history antes da primeira mensagem.
        if path == f"/api/sessions/{SESSION}/commands":
            return self.reply([{"name": "compact", "display": "/compact", "source": "builtin",
                                "description": "Resume e compacta o contexto", "destructive": True}])
        if path == "/api/config":
            return self.reply({"campos": {key: {
                "valor": value, "definido": True, "origem": "env"}
                for key, value in {"notify_finished": notify_finished, "finish_min_seconds": finish_min_seconds}.items()},
                "somente_leitura": {}, "variaveis_env": []})
        if path == "/api/desktop/palette":
            return self.error_body("erro_sem_paleta", "sem paleta", 404)
        self.reply({"detail": "Not Found"}, 404)

    def do_POST(self):
        global state, mode, delay, revision, reconnect, notify_finished, finish_min_seconds
        path = urlsplit(self.path).path
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))) or b"{}")
            if not isinstance(body, dict):
                raise ValueError("control")
            with condition:
                if path == "/fixture/settings":
                    selected = body.get("mode", mode)
                    seconds = float(body.get("delay", 0))
                    enabled = body.get("notify_finished", notify_finished)
                    minimum = body.get("finish_min_seconds", finish_min_seconds)
                    if type(enabled) is not bool or type(minimum) is not int or not 0 <= minimum <= 2**64 - 1:
                        raise ValueError("config")
                    if selected not in ("normal", "muted", "quiet", "error") or not 0 <= seconds <= 60:
                        raise ValueError("settings")
                    mode, delay = selected, seconds
                    notify_finished, finish_min_seconds = enabled, minimum
                elif path == "/fixture/state":
                    selected = body["state"]
                    if selected not in ("working", "idle", "awaiting_input"):
                        raise ValueError("state")
                    state, revision = selected, revision + 1
                elif path == "/fixture/reconnect":
                    reconnect += 1
                else:
                    return self.reply({"detail": "Not Found"}, 404)
                condition.notify_all()
        except (ValueError, KeyError, TypeError):
            return self.reply({"error": "invalid fixture control"}, 400)
        self.server.record(f"{path} state={state} mode={mode} delay={delay} finished={notify_finished} minimum={finish_min_seconds}")
        self.reply({"ok": True})


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    log = (args.output_dir / f"fixture-{server.server_port}.log").open("a", buffering=1)
    server.record = lambda text: log.write(f"{time.time():.3f} {text}\n")
    print(f"http://127.0.0.1:{server.server_port}", flush=True)
    try:
        server.serve_forever()
    finally:
        server.server_close()
        log.close()
