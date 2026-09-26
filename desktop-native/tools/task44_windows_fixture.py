"""Servidor sintético da página Controle do Windows. Nenhum pedido chega ao backend real.

GET /control/t44 aceita get=ok|error, save=ok|error, install=ok|error|skipped,
mode=local|package, agent=present|missing, enabled=0|1 e reset=1.
"""
import json
import pathlib
import threading
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task43_menus_fixture.py").read_text(encoding="utf-8")
T43 = {"__name__": "task43_base", "__file__": str(HERE / "task43_menus_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nif __name__ ==")], "task43_menus_fixture.py", "exec"), T43)
BASE = T43["BASE"]
LOCK = threading.Lock()
PRESET_URL = "http://127.0.0.1:8317/v1/chat/completions"


def initial():
    return {"get": "ok", "save": "ok", "install": "ok", "mode": "local",
            "agent": "present", "enabled": False, "project_dir": "/fixture/project",
            "agent_config": "/fixture/project/delphi-02-agent.json", "installed_tag": ""}


STATE = initial()


def snapshot(skipped=None):
    package = STATE["mode"] == "package"
    root = "/fixture/computer-control" if package else STATE["project_dir"]
    config = STATE["agent_config"]
    target = {"name": "delphi-02", "path": config, "transport": "ssh", "host": "delphi-02"}
    body = {
        "agent_exe": {"path": f"{root}{'' if package else '/dist'}/windows-agent.exe", "exists": STATE["agent"] == "present", "size": 5_242_880 if STATE["agent"] == "present" else 0},
        "mode": STATE["mode"], "installed_tag": STATE["installed_tag"],
        "package_exists": package and STATE["agent"] == "present",
        "targets": [target] if config and STATE["save"] != "error" else [],
        "ssh_hosts": ["delphi-02"], "local_available": False,
        "enabled": STATE["enabled"], "project_dir": "/fixture/Projetos/hangar-computer-control" if package else STATE["project_dir"],
        "agent_config": config, "agent_configs": [target["path"]] if config and STATE["save"] != "error" else [],
        "llm_url": PRESET_URL, "llm_model": "modelo-sintetico", "llm_effort": "medium",
        "llm_key_set": False, "llm_key_tail": "", "jev_key_set": False,
        "jev_key_tail": "", "jev_key_from_settings": False,
        "cliproxy": {"preset_url": PRESET_URL, "has_keys": True,
                      "installed": False, "running": False, "key_is_cliproxy": False},
        "files": [{"path": f"/fixture/.claude{suffix}.json", "enabled": STATE["enabled"]}
                  for suffix in ("", "-conta")],
    }
    if skipped is not None:
        body["migration_skipped"] = ["quebrado-agent.json"] if skipped else []
    return body


def error(code, message, **params):
    return {"detail": {"code": code, "params": params, "msg": message}}


class Handler(T43["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t44":
            query = parse_qs(url.query)
            allowed = {"get": {"ok", "error"}, "save": {"ok", "error"},
                       "install": {"ok", "error", "skipped"}, "mode": {"local", "package"},
                       "agent": {"present", "missing"}, "enabled": {"0", "1"}, "reset": {"1"}}
            if any(key not in allowed or len(values) != 1 or values[0] not in allowed[key] for key, values in query.items()):
                self.send_json({"detail": "controle inválido"}, 400)
                return
            with LOCK:
                if "reset" in query:
                    STATE.clear()
                    STATE.update(initial())
                for key in ("get", "save", "install", "mode", "agent"):
                    if key in query:
                        STATE[key] = query[key][0]
                if "enabled" in query:
                    STATE["enabled"] = query["enabled"][0] == "1"
                if STATE["mode"] == "package" and not STATE["installed_tag"]:
                    STATE["installed_tag"] = "v0.1-sintetica"
                    STATE["agent_config"] = "/fixture/computer-control/targets/delphi-02-agent.json"
                body = snapshot()
            self.send_json(body)
            return
        if url.path == "/api/computer-control":
            if not self.authorized():
                return
            print("GET /api/computer-control", flush=True)
            with LOCK:
                failed, body = STATE["get"] == "error", snapshot()
            if failed:
                self.send_json(error("erro_computer_control_read", "não consegui ler /fixture/.claude.json: JSON inválido",
                                     file="/fixture/.claude.json", error="JSON inválido"), 500)
            else:
                self.send_json(body)
            return
        super().do_GET()

    def do_PUT(self):
        if urlparse(self.path).path != "/api/computer-control":
            return super().do_PUT()
        if not self.authorized():
            return
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
        print(f"PUT /api/computer-control enabled={bool(body['enabled'])} mode={body.get('mode')}", flush=True)
        with LOCK:
            if not body["enabled"]:
                STATE["enabled"] = False
                reply, status = snapshot(), 200
            elif STATE["save"] == "error":
                agent = body["agent_config"]
                reply = error("erro_computer_control_agent", f"o arquivo {agent} não existe", file=agent)
                status = 400
            else:
                STATE["enabled"] = True
                STATE["project_dir"] = body["project_dir"]
                STATE["agent_config"] = body["agent_config"]
                reply, status = snapshot(), 200
        self.send_json(reply, status)

    def do_POST(self):
        if urlparse(self.path).path != "/api/computer-control/install":
            return super().do_POST()
        if not self.authorized():
            return
        print("POST /api/computer-control/install", flush=True)
        with LOCK:
            if STATE["install"] == "error":
                reply = error("erro_computer_control_no_uvx", "o uvx não está no PATH deste servidor (vem com o uv)")
                status = 400
            else:
                skipped = STATE["install"] == "skipped"
                STATE.update(mode="package", installed_tag="v0.1-sintetica", agent="present", enabled=True,
                             agent_config="/fixture/computer-control/targets/delphi-02-agent.json")
                reply, status = snapshot(skipped), 200
        self.send_json(reply, status)


if __name__ == "__main__":
    T43["Handler"] = Handler
    T43["main"]()
