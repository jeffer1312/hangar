"""Synthetic backend for the native chat Task 4 checks: attachments, commands and cited files.

Nothing here talks to a real session. Every mutation is recorded (GET /control/log) and printed.
GET /control/mode?next=<ok|409|413|503|drop|slow> changes how the NEXT mutation answers;
GET /control/state?name=<s>&state=<working|idle|awaiting_input> flips a session state.
GET /control/mode?next=queued makes the next /input answer delivered:false (durable queue, nothing in the transcript).
GET /control/rotate?name=<s> gives the session a new jsonl, as a /clear does.
"""

import json
import os
import struct
import threading
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

TOKEN = "parity-composer-fixture"
LOCK = threading.Lock()
LOG = []
MODE = {"next": "ok"}
UPLOADS = {}
VERSION = {"n": 0}


def png(width, height, rgb):
    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


PDF = b"%PDF-1.1\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n"
HTML = b"<!doctype html><script>document.title='ativo'</script><p>pagina sintetica</p>"
COMMANDS = [
    {"name": "clear", "display": "/clear", "description": "Limpa a conversa", "argumentHint": None, "source": "builtin", "destructive": True},
    {"name": "compact", "display": "/compact", "description": "Compacta o contexto", "argumentHint": "[instruções]", "source": "builtin", "destructive": False},
    {"name": "cost", "display": "/cost", "description": "Mostra o custo da sessão", "argumentHint": None, "source": "builtin", "destructive": False},
    {"name": "model", "display": "/model", "description": "Troca o modelo", "argumentHint": None, "source": "builtin", "destructive": False},
    {"name": "btw", "display": "/btw", "description": "Pergunta lateral", "argumentHint": "<pergunta>", "source": "builtin", "destructive": False},
    {"name": "revisar", "display": "/revisar", "description": "Skill sintética de revisão", "argumentHint": None, "source": "skill", "destructive": False},
    {"name": "plugin-x:rodar", "display": "/plugin-x:rodar", "description": "Comando de plugin sintético", "argumentHint": "<alvo>", "source": "plugin", "destructive": False},
]


def info(name, provider, headless=False, state="idle"):
    return {"name": name, "cwd": f"/synthetic/{name}", "jsonl": f"/synthetic/{name}.jsonl", "provider": provider,
            "headless": headless, "state": state, "tracked": True}


def msg(kind, eid, text, **extra):
    return {"kind": kind, "id": eid, "text": text, **extra}


def build():
    history = [
        msg("user_msg", "u1", "Veja o print e o log — 📎 imagem: /srv/uploads/1790000000-aa11.png 📎 arquivo: /srv/uploads/1790000001-bb22.zip"),
        msg("user_msg", "u2", "Colei direto no terminal", image_count=1),
        msg("assistant_msg", "a1", "Gerei o gráfico em ./out/grafico.png e o relatório em /synthetic/relatorio.pdf. "
            "A prévia está em /synthetic/pagina.html e o caminho /synthetic/fora.png não foi citado pelo servidor."),
        msg("assistant_msg", "a2", f"Nomes longos: /synthetic/{'a' * 115}.html.pdf e /synthetic/{'b' * 115}.pdf.html"),
    ]
    return {
        "compose-claude": {"info": info("compose-claude", "claude"), "state": {"state": "idle"}, "events": list(history), "suggest": "rode os testes agora"},
        "compose-codex": {"info": info("compose-codex", "codex", state="working"), "state": {"state": "working"},
                          "events": [msg("user_msg", "c1", "Comece a migração."), msg("assistant_msg", "c2", "Trabalhando nisso.")], "suggest": ""},
        "compose-headless": {"info": info("compose-headless", "claude", headless=True, state="awaiting_input"), "state": {"state": "awaiting_input"},
                             "events": [msg("user_msg", "h1", "Prepare o ambiente.")], "suggest": ""},
    }


SESSIONS = build()
CITED = {"/synthetic/relatorio.pdf": ("application/pdf", PDF), "./out/grafico.png": ("image/png", png(160, 90, (70, 160, 110))),
         "/synthetic/pagina.html": ("text/html", HTML)}


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

    def send_bytes(self, data, kind, status=200):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def send_json(self, body, status=200):
        self.send_bytes(json.dumps(body, ensure_ascii=False).encode(), "application/json", status)

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
            elif path == "/control/state":
                s = SESSIONS[query["name"][0]]
                s["state"] = {"state": query["state"][0]}
                s["info"]["state"] = query["state"][0]
                bump()
            elif path == "/control/rotate":
                s = SESSIONS[query["name"][0]]
                s["info"]["jsonl"] = f"/synthetic/{s['info']['name']}-{int(time.time() * 1000)}.jsonl"
                bump()
            elif path == "/control/reset":
                SESSIONS.clear()
                SESSIONS.update(build())
                UPLOADS.clear()
                LOG.clear()
                bump()
            self.send_json({"mode": MODE["next"], "log": LOG if path == "/control/log" else len(LOG)})

    def do_GET(self):
        url = urlparse(self.path)
        path, query = url.path, parse_qs(url.query)
        if path.startswith("/control/"):
            self.control(path, query)
            return
        if not self.authorized():
            return
        parts = [unquote(p) for p in path.strip("/").split("/")]
        name = parts[2] if len(parts) > 2 else None
        if path == "/api/sessions":
            with LOCK:
                self.send_json([s["info"] for s in SESSIONS.values()])
        elif path == "/api/sessions/events":
            self.stream_list()
        elif name not in SESSIONS:
            self.send_json({"detail": "not found"}, 404)
        elif parts[3:] == ["history"]:
            with LOCK:
                self.send_json(SESSIONS[name]["events"])
        elif parts[3:] == ["events"]:
            self.stream_session(name)
        elif parts[3:] == ["commands"]:
            record("GET", self.path, None)
            self.send_json(COMMANDS)
        elif parts[3:] == ["uploads"]:
            with LOCK:
                files = [{"filename": f, "size": len(d), "mtime": t, "expires_in_days": 7} for f, (d, t) in UPLOADS.items()]
            files.append({"filename": "1790000000-aa11.png", "size": 2048, "mtime": 1790000000, "expires_in_days": 7})
            self.send_json({"files": files})
        elif parts[3] == "uploads" and len(parts) == 5:
            with LOCK:
                stored = UPLOADS.get(parts[4])
            if stored:
                self.send_bytes(stored[0], "application/octet-stream")
            elif parts[4].endswith(".png"):
                self.send_bytes(png(200, 120, (120, 130, 230)), "image/png")
            elif parts[4].endswith(".zip"):
                self.send_bytes(b"PK\x05\x06" + b"\x00" * 18, "application/zip")
            else:
                self.send_json({"detail": "anexo não encontrado"}, 404)
        elif parts[3] == "transcript-image":
            self.send_bytes(png(140, 100, (220, 150, 80)), "image/png")
        elif parts[3:] == ["file"]:
            cited = query.get("path", [""])[0]
            if cited in CITED:
                kind, data = CITED[cited]
                self.send_bytes(data, kind)
            else:
                self.send_json({"detail": "caminho não citado nesta conversa"}, 403)
        else:
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
                        frames = [("state", s["state"], None), ("ask_question", None, None), ("suggest", {"text": s["suggest"]}, None)]
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
        if action == "upload":
            body = {"x_filename": self.headers.get("X-Filename"), "content_type": self.headers.get("Content-Type"), "bytes": len(raw)}
        else:
            body = json.loads(raw) if raw else None
        record("POST", self.path, body)
        if not self.authorized():
            return
        with LOCK:
            # "queued" espera o próximo /input: o upload que vem antes num envio com anexo não o consome.
            if MODE["next"] == "queued" and action != "input":
                mode = "ok"
            else:
                mode, MODE["next"] = MODE["next"], "ok"
        if mode == "drop":
            self.close_connection = True
            self.connection.shutdown(2)
            return
        if mode == "slow":
            time.sleep(6)
        if mode in ("409", "413", "503"):
            detail = {"409": "a sessão recusou agora", "413": "arquivo maior que 100 MiB", "503": "serviço indisponível"}[mode]
            self.send_json({"detail": {"code": "erro_sintetico", "params": {}, "msg": detail}}, int(mode))
            return
        with LOCK:
            s = SESSIONS.get(parts[2])
            if s is None:
                self.send_json({"detail": "sessão não encontrada"}, 404)
                return
            reply = self.apply(s, action, body, raw, parse_qs(url.query), mode)
            bump()
        self.send_json(reply)

    def apply(self, s, action, body, raw, query, mode="ok"):
        if action == "input" and mode == "queued":
            # Fila durável: aceito, mas não digitado no terminal nem gravado na conversa agora.
            return {"ok": True, "delivered": False}
        if action == "upload":
            ext = unquote(body["x_filename"] or "arquivo").rsplit(".", 1)[-1][:8]
            stored = f"{int(time.time())}-{len(UPLOADS):04x}.{ext}"
            UPLOADS[stored] = (raw, time.time())
            reply: dict = {"path": f"/srv/uploads/{stored}"}
            if ext == "mp4":
                reply.update(frames=[f"/srv/uploads/{stored}-q1.jpg"], transcript="fala sintética do vídeo")
            return reply
        if action in ("input", "steer"):
            text = body["text"]
            # A conversa só mostra o que foi entregue: com anexo, grava só a legenda como o Claude Code.
            shown = text.split(" — 📎")[0] if " — 📎" in text else text
            later(1.5, lambda: s["events"].append(msg("user_msg", f"live-{len(LOG)}", shown)))
            return {"ok": True, "delivered": True} if action == "input" else {"ok": True, "promoted": False}
        if action == "interrupt":
            s["state"] = {"state": "idle"}
            s["info"]["state"] = "idle"
            s["last_clear"] = query.get("clear", ["false"])[0]
            return {"ok": True}
        return {"detail": "rota sintética desconhecida"}


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PARITY_COMPOSER_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port} token={TOKEN}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
