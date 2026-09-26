"""T59: três cartões sintéticos (rodando, concluído, falhou), porta aleatória.

--output-dir guarda log por porta e configs isoladas (dark/light).
Controles herdados: /control/t15?do=bg_task termina o rodando;
/control/t15c?grow=1 traz texto novo; slow=ag-bg1&delay=4 atrasa o detalhe;
/control/t15b?delay=4 atrasa a lista. Nenhum acesso ao backend real.
"""
import argparse
import json
import os
from pathlib import Path
import sys


HERE = Path(__file__).resolve().parent
source = (HERE / "task33_subagents_fixture.py").read_text()
scene = {"__name__": "task59_base", "__file__": str(HERE / "task33_subagents_fixture.py")}
exec(compile(source[:source.index("\nserver = BASE[")], "task33_subagents_fixture.py", "exec"), scene)
base, t15, t15b = scene["BASE"], scene["T15"], scene["T15B"]


def prepare():
    t15["scene_reset"]()
    for step in ("calm", "bg", "fail", "fail_end", "tasks"):
        t15["apply"](step)
    done = next(s for s in t15b["listing"]() if s["agentId"] == "ag-orf1")
    t15["SESSIONS"][t15["NAME"]]["events"].extend([
        t15["agent"]("live-done", "t-done", "Mapear rotas", done["prompt"]),
        t15["result"]("live-done-end", "t-done", "Terminei. agentId: ag-orf1"),
    ])
    for name in list(t15["SESSIONS"]):
        if name not in (t15["NAME"], "sintetica-extra-1"):
            del t15["SESSIONS"][name]


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    prepare()
    server = base["ThreadingHTTPServer"](("127.0.0.1", 0), scene["Handler"])
    address = f"http://127.0.0.1:{server.server_port}"
    for theme in ("dark", "light"):
        config = args.output_dir / theme / "hangar-native"
        config.mkdir(parents=True, exist_ok=True, mode=0o700)
        connection = config / "connection.json"
        connection.write_text(json.dumps({"address": address, "token": base["TOKEN"]}))
        connection.chmod(0o600)
        (config / "appearance.json").write_text(json.dumps({"theme": theme, "language": "pt", "tool_look": "chips"}))
    (args.output_dir / "fixture.json").write_text(json.dumps({"url": address, "pid": os.getpid()}))
    print(f"Fixture URL: {address}", flush=True)
    sys.stdout = (args.output_dir / f"fixture-{server.server_port}.log").open("a", buffering=1)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
