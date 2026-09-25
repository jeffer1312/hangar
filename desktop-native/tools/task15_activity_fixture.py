"""Fixture SINTÉTICA da Task 15b (aba Atividade): a da 15a (task15_working_fixture.py, que não muda) com
`GET /api/sessions/{n}/subagents` e o campo `shells` do estado. Nada daqui existe de verdade; nada sai deste processo.

GET /control/t15b?subs=<modo>[&delay=<s>] muda a resposta de /subagents de TODAS as sessões:
  ok        lista sintética (casa com o Agent de fundo da 15a, órfãos concluído/ilegível e dois de swarm com o mesmo começo)
  empty     []
  404 | 500 erro do backend (o 404 é o "sem sessão ou transcript")
  drop      fecha a conexão sem responder (queda)
  delay=<s> segura a resposta de /subagents por s segundos (resposta atrasada)
GET /control/t15?do=<passo> continua valendo (passos da 15a) e ganha:
  procs | noprocs   processos vivos no estado (`shells`)
  tasks             TaskCreate ×3 + TaskUpdate (em andamento, concluída)
  todo              TodoWrite com a lista inteira (vence as TaskCreate)
Cada pedido a /subagents sai no log como "REQ" (a contagem de 5 s e a leitura da aba aparecem ali).
"""
import pathlib
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task15_working_fixture.py").read_text(encoding="utf-8")
T15 = {"__name__": "task15_base", "__file__": str(HERE / "task15_working_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task15_working_fixture.py", "exec"), T15)

BASE, LOCK, SESSIONS, bump, call, NAME = (T15[k] for k in ("BASE", "LOCK", "SESSIONS", "bump", "call", "NAME"))
record = BASE.get("record") or (lambda method, path, body: print("REQ", method, path, flush=True))

SUBS = {"mode": "ok", "delay": 0.0}
PROCS = {"on": False}


def sub(agent_id, prompt, calls, recent, agent_type="Explore", **extra):
    return {"agentId": agent_id, "agentType": agent_type, "prompt": prompt, "startedAt": "2026-09-25T13:00:00Z",
            "updatedAt": "2026-09-25T13:05:00Z", "mtime": 1790350000.0 + calls, "toolCalls": calls,
            "tools": [{"name": r, "count": 1} for r in recent], "recent": [{"name": r, "target": "/sintetica"} for r in recent],
            "lastText": "", **extra}


def listing():
    return [
        # Casa com o Agent de fundo da 15a pelo prompt inteiro.
        sub("ag-bg1", "Confira conversation.rs (sintético).", 7, ["Read", "Grep"]),
        sub("ag-orf1", "Base directory for this skill: /sintetica/skills/x\n# Título\n\nMapear as rotas sintéticas do backend", 12, ["Read", "Bash"],
            finished=True),
        sub("ag-sw1", "Revise a tela sintética\nitem 1", 3, ["Read"]),
        sub("ag-sw2", "Revise a tela sintética\nitem 2", 4, ["Glob"]),
        {"agentId": "ag-ilegivel", "agentType": None, "prompt": None, "startedAt": "", "updatedAt": "", "mtime": 1790340000.0,
         "toolCalls": 0, "tools": [], "recent": [], "lastText": "", "ilegivel": True},
    ]


STEPS = T15["STEPS"]
STEPS["tasks"] = [
    call("live-k1", "t-k1", "TaskCreate", {"subject": "Ler o ActivitySheet", "activeForm": "Lendo o ActivitySheet"}),
    call("live-k2", "t-k2", "TaskCreate", {"subject": "Codar a aba", "activeForm": "Codando a aba"}),
    call("live-k3", "t-k3", "TaskCreate", {"subject": "Provar na tela"}),
    call("live-k4", "t-k4", "TaskUpdate", {"taskId": "1", "status": "completed"}),
    call("live-k5", "t-k5", "TaskUpdate", {"taskId": "2", "status": "in_progress"}),
]
STEPS["todo"] = [call("live-td", "t-td", "TodoWrite", {"todos": [
    {"content": "Lista inteira do TodoWrite", "status": "completed"},
    {"content": "Segundo item", "activeForm": "Fazendo o segundo item", "status": "in_progress"},
    {"content": "Terceiro item", "status": "pending"}]})]

BASE_APPLY = T15["apply"]


def apply(step):
    if step in ("procs", "noprocs"):
        PROCS["on"] = step == "procs"
        SESSIONS[NAME]["state"] = state_with_procs(SESSIONS[NAME]["state"]["state"])
        T15["LIVE"]["version"] += 1
        bump()
        return True
    return BASE_APPLY(step)


T15["apply"] = apply
BASE_STATE = T15["state"]


def state_with_procs(value="idle", **extra):
    data = BASE_STATE(value, **extra)
    data["shells"] = [{"pid": 4242, "cmd": "npm run dev -- --port 5999 (sintético)", "desde": time.time() - 3900},
                      {"pid": 4243, "cmd": "sleep 900", "desde": None}] if PROCS["on"] else []
    return data


# `set_state` da 15a monta o estado por este nome: com os processos ligados, eles seguem em todo estado novo.
T15["state"] = state_with_procs


class Handler(T15["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t15b":
            query = parse_qs(url.query)
            with LOCK:
                SUBS["mode"] = query.get("subs", [SUBS["mode"]])[0]
                SUBS["delay"] = float(query.get("delay", [SUBS["delay"]])[0])
            self.send_json(dict(SUBS))
            return
        parts = url.path.strip("/").split("/")
        if len(parts) == 4 and parts[:2] == ["api", "sessions"] and parts[3] == "subagents":
            if not self.authorized():
                return
            record("GET", self.path, None)
            with LOCK:
                mode, delay = SUBS["mode"], SUBS["delay"]
            time.sleep(delay)
            if mode == "drop":
                self.close_connection = True
                self.connection.close()
                return
            if mode == "404":
                self.send_json({"detail": "sessão sem transcript (sintético)"}, 404)
            elif mode == "500":
                self.send_json({"detail": "falha sintética"}, 500)
            else:
                self.send_json(listing() if mode == "ok" and parts[2] == NAME else [])
            return
        super().do_GET()



server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
