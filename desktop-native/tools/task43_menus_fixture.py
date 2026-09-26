"""Fixture sintética dos menus: reutiliza T14 sem abrir seu servidor.

Execute com --output apontando para o diretório durável da prova. A pasta por
porta contém config isolada, PID e log; credencial sintética nunca sai dela.
/control/t14 controla atrasos/falhas; /control/r5 controla contas.
/control/t43?branches=long|short&sessions=single|all&cookie=set|clear prepara
os estados extras. Nenhuma operação alcança serviço, conta ou Git real.
"""
import argparse
import contextlib
import json
import os
import pathlib
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task14_sidebar_fixture.py").read_text(encoding="utf-8")
T14 = {"__name__": "task14_base", "__file__": str(HERE / "task14_sidebar_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task14_sidebar_fixture.py", "exec"), T14)
BASE = T14["BASE"]
SHORT_BRANCHES = list(T14["BRANCHES"])


class Handler(T14["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        query = parse_qs(url.query)
        if url.path == "/control/t43":
            allowed = {"branches": {"long", "short"}, "sessions": {"single", "all"}, "cookie": {"set", "clear"}}
            if any(key not in allowed or len(values) != 1 or values[0] not in allowed[key] for key, values in query.items()):
                self.send_json({"detail": "controle inválido"}, 400)
                return
            with BASE["LOCK"]:
                if "branches" in query:
                    T14["BRANCHES"][:] = SHORT_BRANCHES + (
                        [f"sintetica/branch-{n:02}" for n in range(30)] if query["branches"][0] == "long" else [])
                if "sessions" in query:
                    T14["fill"]()
                    if query["sessions"][0] == "single":
                        session = BASE["SESSIONS"]["sintetica-api"]
                        BASE["SESSIONS"].clear()
                        BASE["SESSIONS"]["sintetica-api"] = session
                    BASE["bump"]()
            if "cookie" in query:
                accounts = BASE["accounts"]
                with accounts.LOCK:
                    if query["cookie"][0] == "set":
                        accounts.COOKIES.add("chave:opencode-zen")
                    else:
                        accounts.COOKIES.discard("chave:opencode-zen")
            self.send_json({"ok": True})
            return
        if url.path == "/api/engines":
            if self.authorized():
                BASE["record"]("GET", self.path, None)
                BASE["accounts"].handle_get(self, url.path, query)
            return
        super().do_GET()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
    stage = args.output.resolve() / f"fixture-{server.server_port}"
    stage.mkdir(parents=True, mode=0o700)
    config = stage / "config" / "hangar-native"
    config.mkdir(parents=True, mode=0o700)
    address = f"http://127.0.0.1:{server.server_port}"
    connection = config / "connection.json"
    with open(connection, "x", opener=lambda path, flags: os.open(path, flags, 0o600)) as output:
        json.dump({"address": address, "token": BASE["TOKEN"]}, output)
    (config / "appearance.json").write_text(json.dumps({"theme": "dark", "language": "pt", "navigation": "sidebar"}))
    (stage / "fixture.pid").write_text(str(os.getpid()))
    print(json.dumps({"url": address, "pid": os.getpid(), "stage": str(stage)}), flush=True)
    with (stage / f"fixture-{server.server_port}.log").open("a", buffering=1) as log:
        with contextlib.redirect_stdout(log), contextlib.redirect_stderr(log):
            try:
                server.serve_forever()
            except KeyboardInterrupt:
                pass
            finally:
                server.server_close()


if __name__ == "__main__":
    main()
