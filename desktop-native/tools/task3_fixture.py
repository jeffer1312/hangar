"""Manual, isolated HTTP/SSE fixture for the native Task 3 UI walkthrough."""

import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse


TOKEN = "task3-fixture"
SESSION = "synthetic-alpha"
REVIEW2 = os.environ.get("TASK3_FIXTURE_REVIEW2") == "1"
LOCK = threading.Lock()
RESET = threading.Event()
STATE = {"connections": 0, "history": 0, "cursors": [], "times": [],
         "list_command": 0, "list_event": "", "chat_command": 0, "chat_action": "",
         "denied_connections": 0}


def info():
    return {
        "name": SESSION,
        "cwd": "/synthetic/task3",
        "jsonl": "fixture-a.jsonl",
        "provider": "claude",
        "headless": False,
        "state": "working",
        "tracked": True,
    }


def chat_event(kind, event_id, text, **extra):
    return {"kind": kind, "id": event_id, "text": text, **extra}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def json_response(self, value, status=200):
        body = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def sse_start(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()

    def frame(self, event, value, event_id=None, fragmented=False):
        data = json.dumps(value, ensure_ascii=False)
        if fragmented:
            split = data.index(",") + 1
            fields = [f"data: {data[:split]}", f"data: {data[split:]}"]
        else:
            fields = [f"data: {data}"]
        lines = [": synthetic fixture", f"event: {event}", *fields]
        if event_id is not None:
            lines.append(f"id: {event_id}")
        payload = ("\r\n".join(lines) + "\r\n\r\n").encode()
        chunks = [payload[i:i + 3] for i in range(0, len(payload), 3)] if fragmented else [payload]
        for chunk in chunks:
            self.wfile.write(chunk)
            self.wfile.flush()

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/control/status":
            with LOCK:
                self.json_response(dict(STATE))
            return
        if path == "/control/reset":
            RESET.set()
            self.json_response({"ok": True})
            return
        if path == "/control/list-error":
            with LOCK:
                STATE["list_command"] += 1
                STATE["list_event"] = "list_error"
            self.json_response({"ok": True})
            return
        if path == "/control/list-recover":
            with LOCK:
                STATE["list_command"] += 1
                STATE["list_event"] = "sessions"
            self.json_response({"ok": True})
            return
        if path.startswith("/control/preview-"):
            with LOCK:
                STATE["chat_command"] += 1
                STATE["chat_action"] = path.rsplit("/", 1)[1]
            self.json_response({"ok": True})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.json_response({"detail": "fixture token rejected"}, 401)
            return
        if REVIEW2:
            self.review2(path)
            return
        if path == "/api/sessions":
            self.json_response([info()])
        elif path == "/api/sessions/events":
            self.sse_start()
            try:
                self.frame("sessions", [info()])
                with LOCK:
                    last_command = STATE["list_command"]
                while True:
                    with LOCK:
                        command = STATE["list_command"]
                        event = STATE["list_event"]
                    if command != last_command:
                        last_command = command
                        self.frame(event, [info()] if event == "sessions" else {})
                    self.frame("ping", {})
                    time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        elif path == f"/api/sessions/{SESSION}/history":
            time.sleep(1)
            with LOCK:
                generation = STATE["history"]
            if generation:
                self.json_response([chat_event("assistant_msg", "after-reset", "Conversa nova após reset")])
            else:
                self.json_response([
                    chat_event("user_msg", "history-user", "Pedido inicial sintético"),
                    chat_event("assistant_msg", "history-assistant", "Resposta antiga sintética"),
                ])
        elif path == f"/api/sessions/{SESSION}/events":
            with LOCK:
                STATE["connections"] += 1
                number = STATE["connections"]
                STATE["cursors"].append(self.headers.get("Last-Event-ID"))
                STATE["times"].append(round(time.monotonic(), 2))
            print(f"chat connection {number}: cursor={self.headers.get('Last-Event-ID')!r}", flush=True)
            if number == 3:
                self.send_response(429)
                self.send_header("Content-Type", "application/json")
                self.send_header("Retry-After", "3")
                self.end_headers()
                self.wfile.write(b'{"detail":"fixture retry"}')
                return
            self.sse_start()
            try:
                if number == 1:
                    self.frame("ping", {})
                    self.frame("state", {"state": "working"})
                    self.frame("preview", {"session": SESSION, "text": "## Prévia em voo", "md": True, "full": True, "vivo": True})
                    self.frame("message", chat_event("user_msg", "queued-1", "mensagem duplicada", queued_delivered=True))
                    self.frame("message", chat_event("user_msg", "real-1", "mensagem duplicada"), "fixture-a:10", fragmented=True)
                    self.frame("queue_confirmed", chat_event("user_msg", "queued-1", "mensagem duplicada", queued_confirmed=True))
                    self.frame("message", chat_event("assistant_msg", "live-1", "Resposta em voo concluída"), "fixture-a:20")
                    self.frame("message", chat_event("user_msg", "real-2", "repetida de propósito"), "fixture-a:30")
                    self.frame("message", chat_event("user_msg", "real-3", "repetida de propósito"), "fixture-a:40")
                    self.frame("preview", {"session": SESSION, "text": "**literal**", "md": False, "full": True, "vivo": False})
                    self.frame("state", {"state": "idle"})
                elif number == 2:
                    self.frame("message", chat_event("user_msg", "real-3", "repetida de propósito"), "fixture-a:40")
                    self.frame("message", chat_event("assistant_msg", "resumed", "Recuperada pelo cursor"), "fixture-a:50")
                    with LOCK:
                        last_command = STATE["chat_command"]
                    while not RESET.is_set():
                        with LOCK:
                            command = STATE["chat_command"]
                            action = STATE["chat_action"]
                        if command != last_command:
                            last_command = command
                            self.preview_action(action)
                        self.frame("ping", {})
                        time.sleep(1)
                    with LOCK:
                        STATE["history"] = 1
                    self.frame("reset", {})
                    self.frame("message", chat_event("assistant_msg", "after-reset", "Conversa nova após reset"), "fixture-b:10")
                else:
                    with LOCK:
                        last_command = STATE["chat_command"]
                    while True:
                        with LOCK:
                            command = STATE["chat_command"]
                            action = STATE["chat_action"]
                        if command != last_command:
                            last_command = command
                            self.preview_action(action)
                        self.frame("ping", {})
                        time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        else:
            self.json_response({"detail": "not found"}, 404)

    def review2(self, path):
        denied = {**info(), "name": "synthetic-denied", "jsonl": "fixture-denied.jsonl", "state": "idle"}
        sessions = [info(), denied]
        if path == "/api/sessions":
            self.json_response(sessions)
        elif path == "/api/sessions/events":
            self.sse_start()
            try:
                self.frame("sessions", sessions)
                while True:
                    self.frame("ping", {})
                    time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        elif path == f"/api/sessions/{SESSION}/history":
            time.sleep(1)
            self.json_response([chat_event("user_msg", "real-one", "ok")])
        elif path == f"/api/sessions/{SESSION}/events":
            with LOCK:
                STATE["connections"] += 1
            self.sse_start()
            try:
                self.frame("ping", {})
                self.frame("message", chat_event("user_msg", "queued-one", "ok"))
                self.frame("message", chat_event("user_msg", "queued-two", "ok"))
                self.frame("message", chat_event("user_msg", "real-one", "ok"), "fixture-a:10")
                self.frame("queue_confirmed", chat_event("user_msg", "queued-one", "ok", queued_confirmed=True))
                while True:
                    self.frame("ping", {})
                    time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        elif path == "/api/sessions/synthetic-denied/history":
            self.json_response([])
        elif path == "/api/sessions/synthetic-denied/events":
            with LOCK:
                STATE["denied_connections"] += 1
            self.json_response({"detail": "fixture SSE token rejected"}, 401)
        else:
            self.json_response({"detail": "not found"}, 404)

    def preview_action(self, action):
        if action == "preview-markdown":
            self.frame("state", {"state": "working"})
            self.frame("preview", {"session": SESSION, "text": "## Prévia Markdown", "md": True, "full": True, "vivo": True})
        elif action == "preview-plain":
            self.frame("preview", {"session": SESSION, "text": "**literal sem Markdown**", "md": False, "full": True, "vivo": False})
        elif action == "preview-clear":
            self.frame("preview", {"session": SESSION, "text": "", "md": False, "full": True, "vivo": False})
            self.frame("state", {"state": "idle"})


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("TASK3_FIXTURE_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}  Token: {TOKEN}", flush=True)
server.serve_forever()
