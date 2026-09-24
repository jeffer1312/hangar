"""Isolated HTTP fixture for a manual native delivery walkthrough."""

import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse


TOKEN = "task4-fixture"
LOCK = threading.Lock()
OLD_EVENT = {"kind": "user_msg", "id": "old-real", "text": "uncertain one"}
STATE = {"posts": {}, "last_text": "",
         "events": [OLD_EVENT] if os.environ.get("TASK4_FIXTURE_BACKFILL") == "1" else [],
         "revision": 0, "working": False, "interrupts": 0}


def session(name):
    return {"name": name, "cwd": "/synthetic/task4", "jsonl": f"{name}.jsonl",
            "provider": "codex", "headless": True, "tracked": True,
            "state": "working" if STATE["working"] and name == "synthetic-delivery" else "idle"}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def respond(self, value, status=200):
        body = json.dumps(value, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def stream_start(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

    def frame(self, event, value, event_id=None):
        data = json.dumps(value, ensure_ascii=False)
        lines = [f"event: {event}", f"data: {data}"]
        if event_id is not None:
            lines.append(f"id: {event_id}")
        self.wfile.write(("\n".join(lines) + "\n\n").encode())
        self.wfile.flush()

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/control/status":
            with LOCK:
                self.respond({"posts": dict(STATE["posts"]), "working": STATE["working"],
                              "interrupts": STATE["interrupts"], "events": len(STATE["events"])})
            return
        if path == "/control/confirm":
            with LOCK:
                text = STATE["last_text"]
                event = {"kind": "user_msg", "id": "real-confirmed", "text": text}
                STATE["events"].append(event)
                STATE["revision"] += 1
            self.respond({"ok": bool(text)})
            return
        if path == "/control/echo-new":
            with LOCK:
                STATE["events"].append({"kind": "user_msg", "id": "other-client-new", "text": "uncertain one"})
                STATE["revision"] += 1
            self.respond({"ok": True})
            return
        if path == "/control/working":
            with LOCK:
                STATE["working"] = True
                STATE["revision"] += 1
            self.respond({"ok": True})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.respond({"detail": "fixture token rejected"}, 401)
            return
        if path == "/api/sessions":
            with LOCK:
                self.respond([session("synthetic-delivery"), session("synthetic-other")])
        elif path == "/api/sessions/events":
            self.stream_start()
            try:
                last_revision = -1
                while True:
                    with LOCK:
                        revision = STATE["revision"]
                        sessions = [session("synthetic-delivery"), session("synthetic-other")]
                    if revision != last_revision:
                        self.frame("sessions", sessions)
                        last_revision = revision
                    self.frame("ping", {})
                    time.sleep(0.5)
            except (BrokenPipeError, ConnectionResetError):
                pass
        elif path.endswith("/history"):
            with LOCK:
                self.respond(list(STATE["events"]) if "/synthetic-delivery/" in path else [])
        elif path.endswith("/events"):
            self.stream_start()
            try:
                last_revision = -1
                while True:
                    with LOCK:
                        revision = STATE["revision"]
                        working = STATE["working"]
                        events = list(STATE["events"])
                    if revision != last_revision:
                        self.frame("state", {"state": "working" if working else "idle"})
                        if events and "/synthetic-delivery/" in path:
                            self.frame("message", events[-1], "fixture-delivery:10")
                        last_revision = revision
                    self.frame("ping", {})
                    time.sleep(0.5)
            except (BrokenPipeError, ConnectionResetError):
                pass
        else:
            self.respond({"detail": "not found"}, 404)

    def do_POST(self):
        path = urlparse(self.path).path
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.respond({"detail": "fixture token rejected"}, 401)
            return
        if path == "/api/sessions/synthetic-delivery/input":
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
            text = body.get("text", "")
            with LOCK:
                STATE["posts"][text] = STATE["posts"].get(text, 0) + 1
                STATE["last_text"] = text
            if body.get("steer") is not False:
                self.respond({"detail": "steer must be false"}, 400)
            elif text.startswith("slow accepted"):
                time.sleep(5)
                self.respond({"ok": True, "delivered": True, "steered": False, "native": False})
            elif text.startswith("uncertain"):
                time.sleep(5)
                self.close_connection = True
                self.connection.shutdown(2)
            elif text.startswith("reject"):
                self.respond({"detail": {"code": "fixture_rejected", "params": {},
                                         "msg": "Motivo sintético: envio recusado"}}, 409)
            else:
                self.respond({"detail": "unexpected synthetic input"}, 400)
        elif path == "/api/sessions/synthetic-delivery/interrupt":
            with LOCK:
                STATE["interrupts"] += 1
            self.respond({"detail": {"code": "no_turn", "params": {},
                                     "msg": "Motivo sintético: nenhum turno ativo"}}, 409)
        else:
            self.respond({"detail": "not found"}, 404)


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("TASK4_FIXTURE_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}  Token: {TOKEN}", flush=True)
server.serve_forever()
