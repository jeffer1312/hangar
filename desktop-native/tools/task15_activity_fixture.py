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

15c: GET /api/sessions/{n}/subagents/{id}?events=N (detalhe) e GET /control/t15c?detail=<modo>[&slow=<id>&delay=<s>][&grow=0|1][&reset=1]:
  ok        conversa sintética do subagente (ag-bg1: rodando, com pensamento, ferramentas, erro e markdown; ag-orf1:
            concluído; ag-sw1: 0 chamadas e sem eventos (a lista diz 3); ag-sw2: chamadas sem eventos; ag-ilegivel: ilegível; outro id: 404)
  404 | 500 | drop   erro do backend / queda, em todo pedido de detalhe
  flaky     um pedido sim, um não (500): falha isolada, que não para a consulta
  slow=<id>&delay=<s>  só o detalhe desse subagente espera s segundos (resposta atrasada de um subagente anterior)
  grow=1    cada pedido do ag-bg1 acrescenta uma resposta à conversa dele (a consulta de 2,5 s traz novidade); reset=1 zera

15d: os eventos do detalhe usam ids no formato do `_sub_id` do backend (`U`, `U:1`…). ag-fg1 casa com o Agent de
primeiro plano de /control/t15?do=fgsub (fim: do=fgsub_end, sem agentId, casado pelo prompt); o ag-bg1 termina com do=bg_task.
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
DETAIL = {"mode": "ok", "slow": "", "delay": 0.0, "grow": False, "extra": 0, "count": 0}
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
        # 15d: casa com o Agent de primeiro plano do passo `fgsub` só pelo prompt (o resultado dele não traz agentId).
        sub("ag-fg1", "Conte as rotas do backend sintético.", 3, ["Bash", "Read", "Grep"], agent_type="general-purpose"),
        {"agentId": "ag-ilegivel", "agentType": None, "prompt": None, "startedAt": "", "updatedAt": "", "mtime": 1790340000.0,
         "toolCalls": 0, "tools": [], "recent": [], "lastText": "", "ilegivel": True},
    ]


def ev(kind, eid, text=None, **extra):
    return {"kind": kind, "id": eid, "text": text, **extra}


def use(eid, tid, name, tool_input):
    return ev("tool_use", eid, tool_name=name, tool_input=tool_input, tool_use_id=tid)


def out(eid, tid, text, error=False):
    return ev("tool_result", eid, tool_use_id=tid, result=text, is_error=error)


def detail(agent_id):
    base = next((s for s in listing() if s["agentId"] == agent_id), None)
    if base is None:
        return None
    events = []
    if agent_id == "ag-bg1":
        # Ids como o `_sub_id` do backend: as partes da mesma linha do transcript são `U`, `U:1`, `U:2`…
        events = [
            ev("user_msg", "u-0001", "Confira conversation.rs (sintético)."),
            ev("thinking", "a-0001", "Primeiro leio o fold, depois procuro onde o Agent de fundo fecha."),
            use("a-0001:1", "u1", "Read", {"file_path": "/sintetica/src/conversation.rs"}),
            out("r-0001", "u1", "fn fold_activity(events: &[ChatEvent]) -> Activity {\n    // sintético\n}"),
            ev("assistant_msg", "a-0002", "O fold fecha o agente em **dois** caminhos:\n\n- `task:<id>` no resultado\n"
                                          "- `<task-notification>` na mensagem\n\n## Próximo passo\nConferir a ordem de chegada."),
            use("a-0002:1", "u2", "Grep", {"pattern": "task-notification", "path": "/sintetica/src"}),
            out("r-0002", "u2", "Arquivo sintético não encontrado", error=True),
            use("a-0003", "u3", "Grep", {"pattern": "fold_activity", "path": "/sintetica"}),
        ]
        events += [ev("assistant_msg", f"g-{i:04d}", f"Resposta sintética nova número {i + 1}.") for i in range(DETAIL["extra"])]
    elif agent_id in ("ag-orf1", "ag-fg1"):
        events = [ev("user_msg", "u-0101", base["prompt"]),
                  ev("assistant_msg", "a-0101", "Vou contar as rotas."),
                  use("a-0101:1", "v1", "Bash", {"command": "rg '@app.get' /sintetica/backend | wc -l"}),
                  use("a-0101:2", "v2", "Read", {"file_path": "/sintetica/backend/api.py"}),
                  use("a-0101:3", "v3", "Grep", {"pattern": "@app.post", "path": "/sintetica/backend"}),
                  out("r-0101", "v1", "42"), out("r-0101:1", "v2", "rotas sintéticas"), out("r-0101:2", "v3", "7"),
                  ev("assistant_msg", "a-0102", "São 42 rotas sintéticas. Terminei.")]
    elif agent_id == "ag-sw1":
        # Recomeçou: nenhuma chamada ainda ("Ainda pensando"), embora a lista, mais velha, diga 3.
        base = {**base, "toolCalls": 0, "tools": [], "recent": []}
    return {**base, "events": events}


STEPS = T15["STEPS"]
STEPS["tasks"] = [
    call("live-k1", "t-k1", "TaskCreate", {"subject": "Ler o ActivitySheet", "activeForm": "Lendo o ActivitySheet"}),
    call("live-k2", "t-k2", "TaskCreate", {"subject": "Codar a aba", "activeForm": "Codando a aba"}),
    call("live-k3", "t-k3", "TaskCreate", {"subject": "Provar na tela"}),
    call("live-k4", "t-k4", "TaskUpdate", {"taskId": "1", "status": "completed"}),
    call("live-k5", "t-k5", "TaskUpdate", {"taskId": "2", "status": "in_progress"}),
]
# 15d: o subagente do Claude termina com o Agent do pai — pelo prompt (`fgsub`/`fgsub_end`) ou pelo agentId (`bg`/`bg_task`
# da 15a). O `fg` da 15a segue sem subagente no disco (o "não achei" da 15c).
STEPS["fgsub"] = [T15["agent"]("live-fgsub", "t-fgsub", "Contar as rotas", "Conte as rotas do backend sintético.")]
STEPS["fgsub_end"] = [T15["result"]("live-fgsub-end", "t-fgsub", "São 42 rotas sintéticas.")]
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
        if url.path == "/control/t15c":
            query = parse_qs(url.query)
            with LOCK:
                DETAIL["mode"] = query.get("detail", [DETAIL["mode"]])[0]
                DETAIL["slow"] = query.get("slow", [DETAIL["slow"]])[0]
                DETAIL["delay"] = float(query.get("delay", [DETAIL["delay"]])[0])
                DETAIL["grow"] = query.get("grow", ["1" if DETAIL["grow"] else "0"])[0] == "1"
                if "reset" in query:
                    DETAIL["extra"] = 0
                self.send_json(dict(DETAIL))
            return
        parts = url.path.strip("/").split("/")
        if len(parts) == 5 and parts[:2] == ["api", "sessions"] and parts[3] == "subagents":
            if not self.authorized():
                return
            record("GET", self.path, None)
            with LOCK:
                mode, slow, delay = DETAIL["mode"], DETAIL["slow"], DETAIL["delay"]
                DETAIL["count"] += 1
                if DETAIL["grow"] and parts[4] == "ag-bg1":
                    DETAIL["extra"] += 1
                flaky_fail = mode == "flaky" and DETAIL["count"] % 2 == 0
            if slow == parts[4]:
                time.sleep(delay)
            if mode == "drop":
                self.close_connection = True
                self.connection.close()
                return
            body = detail(parts[4]) if parts[2] == NAME else None
            if mode == "404" or body is None:
                self.send_json({"detail": "subagent not found (sintético)"}, 404)
            elif mode == "500" or flaky_fail:
                self.send_json({"detail": "falha sintética do detalhe"}, 500)
            else:
                self.send_json(body)
            return
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
