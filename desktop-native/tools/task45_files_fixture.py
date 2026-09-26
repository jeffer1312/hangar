#!/usr/bin/env python3
"""Prova isolada: --dir DURAVEL; app com XDG_CONFIG_HOME=DURAVEL/config.

Corpos: api.files_resolver/_erro_arq/serve_file_text, filetree.read_at e git_ops.changed_files.
GET /control?delay=3 regula slow.txt; nenhum POST além de files/resolver é aceito.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

LIMIT = 512 * 1024
FILES = {
    "demo.rs": "".join(f'const VALUE_{i}: u32 = {i};' + (" // MARCADOR LINHA 120" if i == 120 else "") + "\n" for i in range(1, 161)),
    "empty.txt": "", "binary.bin": "\x00binario", "truncated.txt": "linha longa para leitura\n" * 24000,
    "plain.unknown": "Texto sem gramática.\nSegunda linha.\n", "slow.txt": "Resposta lenta concluída.\n",
    "/outside/note.txt": "Arquivo citado fora da raiz.\n", "gone.txt": None, "denied.txt": None,
}
SESSIONS = [{"name": name, "cwd": "/fixture/project", "jsonl": f"/fixture/{name}.jsonl", "provider": "claude",
             "headless": True, "state": "idle", "tracked": True, "branch": "fixture", "git_dirty": 9}
            for name in ("t45-arquivos", "t45-outra")]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def record(self, status):
        with self.server.log.open("a") as out:
            out.write(json.dumps({"time": time.time(), "method": self.command, "path": self.path, "status": status}) + "\n")

    def respond(self, body, status=200):
        self.record(status)
        raw = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        try:
            self.wfile.write(raw)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def allowed(self):
        if self.headers.get("Authorization") == f"Bearer {self.server.token}":
            return True
        self.respond({"detail": "Not authenticated"}, 401)
        return False

    def file_error(self, code, status):
        msg = "Nao deu pra acessar esse arquivo ou pasta."
        self.respond({"detail": {"code": code, "params": {"msg": msg}, "msg": msg}}, status)

    def do_POST(self):
        if not self.allowed(): return
        if not urlsplit(self.path).path.endswith("/files/resolver"):
            return self.respond({"detail": "fixture: escrita recusada"}, 405)
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        ok = {}
        for path in body.get("caminhos", []):
            target = "/outside/note.txt" if path == "note.txt" else path.removeprefix("/fixture/project/")
            if target in FILES:
                external = target.startswith("/")
                ok[path] = {"relativo": None if external else target, "real": target if external else "/fixture/project/" + target}
        self.respond({"ok": ok, "faltam": [p for p in body.get("caminhos", []) if p not in ok]})

    def do_GET(self):
        parsed = urlsplit(self.path)
        route, query = parsed.path, parse_qs(parsed.query)
        if route == "/control":
            self.server.delay = max(0, min(35, float(query.get("delay", ["2"])[0])))
            return self.respond({"delay": self.server.delay})
        if not self.allowed(): return
        if route.endswith("/events"):
            self.record(200)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            event, body = ("sessions", SESSIONS) if route == "/api/sessions/events" else ("state", {"state": "idle", "session": route.split("/")[-2]})
            try:
                while True:
                    self.wfile.write(f"event: {event}\ndata: {json.dumps(body)}\n\n".encode())
                    self.wfile.flush()
                    event, body = "ping", {}
                    time.sleep(2)
            except (BrokenPipeError, ConnectionResetError):
                return
        if route == "/api/sessions": return self.respond(SESSIONS)
        if route == "/api/config": return self.respond({"campos": {}, "somente_leitura": {}, "variaveis_env": []})
        if route.endswith("/history"):
            return self.respond([{"kind": "assistant_msg", "id": "fixture-chat", "text": "Abra demo.rs no Projeto.\n\nArquivo externo citado: /outside/note.txt"}])
        if route.endswith("/commands"): return self.respond([])
        if route.endswith("/git/files"):
            return self.respond({"files": [{"path": p, "code": " D" if p in ("note.txt", "missing.txt") else "??", "staged": False, "added": None, "removed": None}
                                           for p in (*[p for p in FILES if not p.startswith("/")], "note.txt", "missing.txt")], "sequencer": None})
        if route.endswith(("/files/read", "/file/text")):
            path = query.get("path", [""])[0]
            target = "/outside/note.txt" if path == "note.txt" else path
            if route.endswith("/files/read") and path.startswith("/"): return self.file_error("erro_arq_caminho_invalido", 400)
            if route.endswith("/files/read") and path == "note.txt": return self.file_error("erro_arq_inexistente", 404)
            if route.endswith("/file/text") and target != "/outside/note.txt":
                return self.respond({"detail": {"code": "erro_arquivo_nao_citado", "params": {}, "msg": "file not referenced in this conversation"}}, 403)
            if target == "slow.txt": time.sleep(self.server.delay)
            if target == "denied.txt": return self.file_error("erro_arq_sem_permissao", 403)
            if FILES.get(target) is None: return self.file_error("erro_arq_inexistente", 404)
            raw = FILES[target].encode()
            if b"\x00" in raw[:LIMIT + 1]: return self.file_error("erro_arq_binario", 415)
            return self.respond({"path": path, "text": raw[:LIMIT].decode("utf-8", errors="replace"), "size": len(raw),
                                 "truncated": len(raw) > LIMIT, "digest": None if len(raw) > LIMIT else hashlib.sha256(raw).hexdigest()})
        self.respond({"detail": "fixture: rota não configurada"}, 404)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    args = parser.parse_args()
    args.dir = args.dir.resolve()
    os.umask(0o077)
    config = args.dir / "config" / "hangar-native"
    config.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.token, server.delay = secrets.token_hex(24), 2
    server.log = args.dir / f"fixture-{server.server_port}.log"
    address = f"http://127.0.0.1:{server.server_port}"
    (config / "connection.json").write_text(json.dumps({"address": address, "token": server.token}))
    (config / "appearance.json").write_text(json.dumps({"theme": "dark", "language": "pt"}))
    (args.dir / "fixture.json").write_text(json.dumps({"pid": os.getpid(), "port": server.server_port, "address": address}))
    print(json.dumps({"pid": os.getpid(), "address": address, "config": str(config.parent)}), flush=True)
    server.serve_forever()
