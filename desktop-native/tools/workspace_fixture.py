#!/usr/bin/env python3
"""Interface real contra o servidor Rust e repositórios descartáveis; sem backend de produção."""
import argparse
import json
import os
import subprocess
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit, unquote

import task70_new_chat_send_fixture as base

NAME = "workspace-fixture"
TOKEN = "workspace-fixture-token"
SECRET = "workspace-fixture-internal"


class Handler(base.Handler):
    def do_OPTIONS(self):
        self.send_response(200)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.end_headers()

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/internal/workspace/context":
            if self.headers.get("x-hangar-internal") != SECRET:
                return self.reply({}, 404)
            return self.reply({"roots":[str(self.server.repo.parent)], "sessions":[{"name":NAME,"cwd":str(self.server.repo)}],
                               "session":{"name":NAME,"cwd":str(self.server.repo),"jsonl":str(self.server.transcript)}})
        if path == "/fixture/proof":
            return self.reply({"python_domain_calls":self.server.domain_calls, "repo":str(self.server.repo)})
        if self.domain(path):
            self.server.domain_calls += 1
            return self.reply({"detail":"A fixture Python não atende arquivos nem Git."}, 500)
        if not path.startswith("/api/") and self.server.frontend is not None:
            target = (self.server.frontend / (unquote(path).lstrip("/") or "index.html")).resolve()
            if target.is_relative_to(self.server.frontend) and target.is_file():
                import mimetypes
                data = target.read_bytes()
                self.send_response(200)
                self.send_header("Content-Type", mimetypes.guess_type(target)[0] or "application/octet-stream")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
        if path == "/api/whoami":
            return self.reply({"kind":"owner"})
        if path == "/api/peers":
            return self.reply([])
        return super().do_GET()

    def do_POST(self):
        if self.domain(urlsplit(self.path).path):
            self.server.domain_calls += 1
            return self.reply({"detail":"A fixture Python não atende arquivos nem Git."}, 500)
        return super().do_POST()

    @staticmethod
    def domain(path):
        prefix = f"/api/sessions/{NAME}/"
        return path.startswith("/api/fs/") or path.startswith(prefix) and (
            path[len(prefix):].startswith(("git", "files/", "file", "branches", "checkout")))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--server-bin", type=Path, required=True)
    parser.add_argument("--frontend-dist", type=Path)
    parser.add_argument("--bind", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=18865)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    repo = output / "repo"
    repo.mkdir(exist_ok=True)
    remote = output / "origin.git"
    os.environ.update({"GIT_CONFIG_GLOBAL":os.devnull, "GIT_CONFIG_NOSYSTEM":"1", "LC_ALL":"C"})
    def git(cwd, *argv):
        subprocess.run(["git", "-C", str(cwd), *argv], check=True, capture_output=True)
    if not remote.exists():
        git(output, "init", "--bare", "-b", "main", str(remote))
    if not (repo / ".git").exists():
        git(repo, "init", "-b", "main")
        git(repo, "config", "user.name", "Fixture")
        git(repo, "config", "user.email", "fixture@example.invalid")
        git(repo, "remote", "add", "origin", str(remote))
        (repo / "src").mkdir()
        (repo / "README.md").write_text("# Validação\n\nRepositório descartável.\n", encoding="utf-8", newline="\n")
        (repo / "src/ação.txt").write_text("Primeira linha.\n", encoding="utf-8", newline="\n")
        git(repo, "add", ".")
        git(repo, "commit", "-m", "Inicial")
        git(repo, "push", "-u", "origin", "main")
        (repo / "src/ação.txt").write_text("Primeira linha.\nAlteração para conferir.\n", encoding="utf-8", newline="\n")
        (repo / "notas.txt").write_text("Arquivo novo.\n", encoding="utf-8", newline="\n")
    transcript = output / "transcript.jsonl"
    transcript.write_text(json.dumps({"cwd":str(repo),"text":"Leia README.md e src/ação.txt."}, ensure_ascii=False), encoding="utf-8")
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.repo, server.transcript = repo, transcript
    server.frontend = args.frontend_dist.resolve() if args.frontend_dist else None
    server.domain_calls = 0
    server.lock = threading.Lock()
    server.control = output / "fixture.case"
    server.control.write_text("success", encoding="utf-8")
    server.sessions = {NAME:{**base.SESSION,"name":NAME,"cwd":str(repo),"jsonl":str(transcript),"branch":"main"}}
    server.history = {NAME:[{"id":"u1","kind":"user_msg","text":"Confira os arquivos e o Git desta pasta.","ts":1790445600.0},
                            {"id":"a1","kind":"assistant_msg","text":"Abra `README.md` e `src/ação.txt` pela aba Arquivos.","ts":1790445601.0}]}
    base.ROOT = str(repo.parent)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    env = {**os.environ,"HANGAR_SERVER_LISTEN":f"{args.bind}:{args.port}","HANGAR_SERVER_UPSTREAM":f"127.0.0.1:{server.server_port}",
           "CP_AUTH_TOKEN":TOKEN,"CP_FORWARDED_ALLOW_IPS":"127.0.0.1","HANGAR_INTERNAL_SECRET":SECRET,
           "HANGAR_SERVER_LOG":str(output / "server.log")}
    child = subprocess.Popen([str(args.server_bin.resolve())], stdin=subprocess.PIPE, env=env)
    print(json.dumps({"address":f"http://{args.bind}:{args.port}","token":TOKEN,"repo":str(repo),"pid":child.pid}), flush=True)
    try:
        child.wait()
    finally:
        if child.stdin:
            child.stdin.close()
        server.shutdown()


if __name__ == "__main__":
    main()
