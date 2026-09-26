#!/usr/bin/env python3
"""Prova isolada da edição; --dir DURAVEL e XDG_CONFIG_HOME=DURAVEL/config.

Corpos de arquivo saem de filetree.read_at/write_at; erros de Handler.file_error
reproduzem api._erro_arq. Demais rotas reutilizam a fixture de leitura.
GET /control?mode=conflict|oversize|binary|normal&delay=2 altera só o palco.
reset=1 restaura apenas o arquivo sintético indicado por path.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import sys
import threading
import time
from http.server import ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

sys.dont_write_bytecode = True
from task45_files_fixture import FILES, Handler as ReadHandler

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "backend"))
from app.filetree import FileError, MAX_BYTES, read_at, write_at


class Handler(ReadHandler):
    def allowed(self):
        if self.headers.get("Authorization") == f"Bearer {self.server.token}": return True
        self.respond({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}}, 401)
        return False

    def target(self, path):
        key = "/outside/note.txt" if path == "note.txt" else path
        return self.server.files.get(key)

    def valid_path(self, route, path):
        if route.endswith("/file/text"):
            if path in ("demo.rs", "note.txt", "/outside/note.txt"): return True
            self.respond({"detail": {"code": "erro_arquivo_nao_citado", "params": {},
                                      "msg": "file not referenced in this conversation"}}, 403)
        elif path.startswith("/"):
            self.file_error("erro_arq_caminho_invalido", 400)
        elif path == "denied.txt":
            self.file_error("erro_arq_sem_permissao", 403)
        elif path != "note.txt":
            return True
        else:
            self.file_error("erro_arq_inexistente", 404)
        return False

    def do_GET(self):
        parsed = urlsplit(self.path)
        query = parse_qs(parsed.query)
        if parsed.path == "/control":
            self.server.delay = max(0, min(25, float(query.get("delay", ["0"])[0])))
            self.server.mode = query.get("mode", ["normal"])[0]
            if self.server.mode in ("conflict", "oversize") or query.get("reset") == ["1"]:
                path = query.get("path", ["/outside/note.txt"])[0]
                target = self.target(path)
                if target is None: return self.file_error("erro_arq_inexistente", 404)
                with self.server.lock:
                    if query.get("reset") == ["1"]:
                        target.write_text(FILES["/outside/note.txt" if path == "note.txt" else path])
                    else:
                        target.write_text("ALTERADO NO DISCO\n" if self.server.mode == "conflict" else "x" * (MAX_BYTES + 1))
            return self.respond({"mode": self.server.mode, "delay": self.server.delay})
        if parsed.path.endswith(("/files/read", "/file/text")):
            if not self.allowed(): return
            path = query.get("path", [""])[0]
            if not self.valid_path(parsed.path, path): return
            target = self.target(path)
            if target is None: return self.file_error("erro_arq_inexistente", 404)
            try:
                with self.server.lock:
                    result = read_at(target, path)
                return self.respond(result)
            except FileError as error:
                return self.file_error(error.code, error.status)
        super().do_GET()

    def do_POST(self):
        route = urlsplit(self.path).path
        if not route.endswith(("/file/text", "/files/write")):
            return super().do_POST()
        if not self.allowed(): return
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        path = body.get("path", "")
        if not self.valid_path(route, path): return
        target = self.target(path)
        if target is None: return self.file_error("erro_arq_inexistente", 404)
        time.sleep(self.server.delay)
        try:
            with self.server.lock:
                if self.server.mode == "binary":
                    raise FileError(415, "erro_arq_binario", "arquivo binario")
                result = write_at(target, path, body["text"], body.get("digest"))
                with self.server.log.open("a") as out:
                    out.write(json.dumps({"saved": path, "read_digest": body.get("digest"), **result}) + "\n")
            self.respond(result)
        except FileError as error:
            self.file_error(error.code, error.status)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    args = parser.parse_args()
    root = args.dir.resolve()
    os.umask(0o077)
    config = root / "config" / "hangar-native"
    config.mkdir(parents=True, exist_ok=True)
    synthetic = root / "synthetic"
    synthetic.mkdir(exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.files = {}
    for index, (name, text) in enumerate(FILES.items()):
        if text is None: continue
        target = synthetic / f"file-{index}"
        target.write_text(text)
        server.files[name] = target
    server.token = secrets.token_hex(24)
    server.delay, server.mode, server.lock = 0, "normal", threading.Lock()
    server.log = root / f"fixture-{server.server_port}.log"
    address = f"http://127.0.0.1:{server.server_port}"
    (config / "connection.json").write_text(json.dumps({"address": address, "token": server.token}))
    (config / "appearance.json").write_text(json.dumps({"theme": "dark", "language": "pt"}))
    (root / "fixture.json").write_text(json.dumps({"pid": os.getpid(), "port": server.server_port, "address": address}))
    print(json.dumps({"pid": os.getpid(), "address": address, "config": str(config.parent)}), flush=True)
    server.serve_forever()
