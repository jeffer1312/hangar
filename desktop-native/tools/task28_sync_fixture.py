"""Servidor sintético da página Sincronização; nenhuma rota alcança o backend em uso.

GET /control/t28?state=<fresh|fresh_on|off|on>&load=<ok|500|drop>&write=<ok|500|drop>&delay=<segundos>
controla as próximas respostas. Um POST que cai após a gravação ainda muda o estado, para provar
que a página relê antes de permitir nova tentativa. GET /control/log registra método e rota.
"""
import os
import pathlib
import time
import base64
import hashlib
import hmac
import json
from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task14_sidebar_fixture.py").read_text(encoding="utf-8")
T14NS = {"__name__": "task14_base", "__file__": str(HERE / "task14_sidebar_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task14_sidebar_fixture.py", "exec"), T14NS)

BASE = T14NS["BASE"]
LOCK, record = BASE["LOCK"], BASE["record"]
MODES = {"state": "off", "load": "ok", "write": "ok", "delay": 0.0,
         "user": "pessoa-sintetica", "derived": None, "encrypted": None}
SYNTHETIC_PASSWORD = "senha-sintetica-28"


def setup():
    state = MODES["state"]
    return {"enabled": state in ("on", "fresh_on"), "registered": state not in ("fresh", "fresh_on"),
            "user": None if state in ("fresh", "fresh_on") else MODES["user"]}


def registration_checks(body):
    try:
        salt = base64.b64decode(body["salt"], validate=True)
        auth = base64.b64decode(body["auth_hash"], validate=True)
        iv = base64.b64decode(body["enc_blob"]["iv"], validate=True)
        data = base64.b64decode(body["enc_blob"]["data"], validate=True)
        master = hashlib.pbkdf2_hmac("sha256", SYNTHETIC_PASSWORD.encode(), salt, 600_000)
        prk = hmac.digest(bytes(32), master, "sha256")
        expected = hmac.digest(prk, b"cp-auth\x01", "sha256")
        enc_key = hmac.digest(prk, b"cp-enc\x01", "sha256")
        servers = json.loads(AESGCM(enc_key).decrypt(iv, data, None))
        listed = (isinstance(servers, list) and len(servers) == 1 and
                  servers[0].get("token") == BASE["TOKEN"] and
                  servers[0].get("baseUrl") == f"http://127.0.0.1:{server.server_port}" and
                  bool(servers[0].get("id")) and bool(servers[0].get("label")))
        return (len(salt) == 16 and hmac.compare_digest(auth, expected), listed)
    except (KeyError, TypeError, ValueError, IndexError, AttributeError, InvalidTag):
        return (False, False)


class Handler(T14NS["Handler"]):
    def failure(self):
        data = b"Internal Server Error"
        self.send_response(500)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def drop(self):
        self.close_connection = True

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t28":
            query = parse_qs(url.query)
            with LOCK:
                for key, values in query.items():
                    value = values[0]
                    if key == "state" and value in ("fresh", "fresh_on", "off", "on"):
                        MODES[key] = value
                        if value in ("fresh", "fresh_on"):
                            MODES.update(user="pessoa-sintetica", derived=None, encrypted=None)
                    elif key == "load" and value in ("ok", "500", "drop"):
                        MODES[key] = value
                    elif key == "write" and value in ("ok", "500", "drop"):
                        MODES[key] = value
                    elif key == "delay":
                        MODES[key] = min(max(float(value), 0.0), 10.0)
                current = dict(MODES)
            self.send_json(current)
            return
        if url.path != "/api/sync/setup":
            return super().do_GET()
        record("GET", self.path, None)
        if not self.authorized():
            return
        with LOCK:
            mode, delay, current = MODES["load"], MODES["delay"], setup()
        time.sleep(delay)
        if mode == "drop":
            self.drop()
        elif mode == "500":
            self.failure()
        else:
            self.send_json(current)

    def do_POST(self):
        path = urlparse(self.path).path
        if path not in ("/api/sync/setup", "/api/sync/setup/disable"):
            return super().do_POST()
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        record("POST", path, None)
        if not self.authorized():
            return
        with LOCK:
            mode, delay = MODES["write"], MODES["delay"]
            fresh = MODES["state"] in ("fresh", "fresh_on")
            body = json.loads(raw or b"{}") if path == "/api/sync/setup" else {}
            registering = fresh and bool(body.get("user"))
            if registering:
                MODES["derived"], MODES["encrypted"] = registration_checks(body)
            valid = not registering or (MODES["derived"] and MODES["encrypted"])
            if valid and mode in ("ok", "drop") and path.endswith("disable"):
                MODES["state"] = "off" if not fresh else "fresh"
            elif valid and mode in ("ok", "drop") and (not fresh or registering):
                MODES["state"] = "on"
                if registering:
                    MODES["user"] = body["user"]
            current = setup()
        time.sleep(delay)
        if not valid:
            self.failure()
        elif mode == "drop":
            self.drop()
        elif mode == "500":
            self.failure()
        elif fresh and not registering and path == "/api/sync/setup":
            self.send_json({"detail": "Informe os dados da conta"}, 422)
        else:
            self.send_json(current)


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK28_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
