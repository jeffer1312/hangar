#!/usr/bin/env python3
"""Interface real contra o servidor Rust e repositórios descartáveis; sem backend de produção."""
import argparse
import json
import os
import subprocess
from pathlib import Path

TOKEN = "workspace-fixture-token"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--server-bin", type=Path, required=True)
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
    env = {**os.environ,"CP_PORT":str(args.port),"CP_LAN_BIND_IP":args.bind,
           "CP_AUTH_TOKEN":TOKEN,"CP_FORWARDED_ALLOW_IPS":"127.0.0.1"}
    child = subprocess.Popen([str(args.server_bin.resolve())], stdin=subprocess.PIPE, env=env)
    print(json.dumps({"address":f"http://{args.bind}:{args.port}","token":TOKEN,"repo":str(repo),"pid":child.pid}), flush=True)
    try:
        child.wait()
    finally:
        if child.stdin:
            child.stdin.close()


if __name__ == "__main__":
    main()
