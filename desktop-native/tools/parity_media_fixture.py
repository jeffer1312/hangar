"""Synthetic backend for the native Task 8 memory check: many sessions, each citing many large images.

Nothing here talks to a real session. Every image is distinct (own bytes, so GPUI cannot dedupe it)
and big enough (1920x1200) that a full decode costs ~9 MB, so unbounded growth shows up in RSS.
GET /control/log lists the image fetches.
"""

import json
import os
import struct
import threading
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

TOKEN = "parity-media-fixture"
SESSIONS_COUNT = int(os.environ.get("PARITY_MEDIA_SESSIONS", "8"))
IMAGES_PER_SESSION = int(os.environ.get("PARITY_MEDIA_IMAGES", "8"))
WIDTH, HEIGHT = 1920, 1200
LOCK = threading.Lock()
CACHE = {}
LOG = []


def png(seed):
    # Faixas de cor por semente, com um texto de linhas finas: detalhe que borra se a miniatura for pequena demais.
    r, g, b = (seed * 53) % 200 + 40, (seed * 97) % 200 + 40, (seed * 31) % 200 + 40
    rows = []
    for y in range(HEIGHT):
        band = (y * 8 // HEIGHT)
        base = bytes((min(255, r + band * 6), min(255, g + band * 4), b))
        line = bytearray(base * WIDTH)
        if (y // 6) % 2 == 0:
            for x in range(seed % 40, WIDTH, 40):
                line[x * 3:x * 3 + 3] = b"\xff\xff\xff"
        rows.append(b"\x00" + bytes(line))
    raw = b"".join(rows)
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 1)) + chunk(b"IEND", b"")


def image(path):
    with LOCK:
        if path not in CACHE:
            CACHE[path] = png(zlib.crc32(path.encode()) % 10_000)
        return CACHE[path]


def name(n):
    return f"fotos-{n:02d}"


def paths(n):
    return [f"/synthetic/{name(n)}/captura-{i:02d}-ação.png" for i in range(IMAGES_PER_SESSION)]


def info(n):
    return {"name": name(n), "cwd": f"/synthetic/{name(n)}", "jsonl": f"/synthetic/{name(n)}.jsonl", "provider": "claude",
            "headless": False, "state": "idle", "tracked": True}


def history(n):
    cited = "".join(f"\n- {p}" for p in paths(n))
    return [
        {"kind": "user_msg", "id": f"u{n}", "text": f"Mostre as capturas da sessão {n} — atenção à ação e à configuração."},
        {"kind": "assistant_msg", "id": f"a{n}", "text": f"Aqui estão as {IMAGES_PER_SESSION} capturas geradas: {cited}"},
    ]


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

    def frame(self, event, data):
        self.wfile.write(f"event: {event}\ndata: {json.dumps(data, ensure_ascii=False)}\n\n".encode())
        self.wfile.flush()

    def do_GET(self):
        url = urlparse(self.path)
        path, query = url.path, parse_qs(url.query)
        if path == "/control/log":
            with LOCK:
                self.send_json({"fetches": len(LOG), "log": LOG[-50:]})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.send_json({"detail": "token"}, 401)
            return
        parts = [unquote(p) for p in path.strip("/").split("/")]
        index = next((n for n in range(SESSIONS_COUNT) if len(parts) > 2 and parts[2] == name(n)), None)
        if path == "/api/sessions":
            self.send_json([info(n) for n in range(SESSIONS_COUNT)])
        elif path == "/api/sessions/events":
            self.stream({"sessions": [info(n) for n in range(SESSIONS_COUNT)]})
        elif index is None:
            self.send_json({"detail": "not found"}, 404)
        elif parts[3:] == ["history"]:
            self.send_json(history(index))
        elif parts[3:] == ["events"]:
            self.stream({"state": {"state": "idle"}, "ask_question": None, "suggest": {"text": ""}})
        elif parts[3:] == ["commands"]:
            self.send_json([])
        elif parts[3:] == ["uploads"]:
            self.send_json({"files": []})
        elif parts[3:] == ["file"]:
            cited = query.get("path", [""])[0]
            if cited in paths(index):
                with LOCK:
                    LOG.append({"t": round(time.time(), 3), "path": cited})
                self.send_bytes(image(cited), "image/png")
            else:
                self.send_json({"detail": "caminho não citado nesta conversa"}, 403)
        else:
            self.send_json({"detail": "not found"}, 404)

    def stream(self, first):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            for event, body in first.items():
                self.frame(event, body)
            while True:
                time.sleep(1)
                self.frame("ping", {})
        except (BrokenPipeError, ConnectionResetError):
            pass


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PARITY_MEDIA_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port} token={TOKEN}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
