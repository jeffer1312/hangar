"""Fixture SINTÉTICA da Task 15a ("trabalhando" no fim da conversa, na barra e nas abas; cartões fixos dos agentes):
a da Task 22 (que carrega a da 14 e a da 13) com uma sessão de roteiro e sessões a mais para a barra rolar.
Nenhuma sessão, pasta, agente ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 22 sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.

Sessão de roteiro: sintetica-agentes. Cada GET /control/t15?do=<passo>[,<passo>…] muda o que o SSE dela manda:
  working | idle             estado sem rótulo
  label                      trabalhando com o rótulo do terminal "Computing… (Ns)", o N sobe a cada segundo
  thinking | nothinking      pensamento ao vivo (evento `pensamento`) liga/desliga
  tool | notool              ferramenta ao vivo (evento `ferramenta`) liga/desliga
  preview | nopreview        prévia ao vivo liga/desliga
  fg | fg_end                Agent em primeiro plano / o resultado dele
  bg | bg_task               Agent em segundo plano com "Async agent launched … agentId: ag-bg1" / fim por tool_result task:ag-bg1
  bg2 | bg2_note | bg2_launch  Agent em segundo plano cujo fim (<task-notification> ag-bg2) chega ANTES do lançamento
  swarm | swarm_end          AgentSwarm com 3 itens / o resultado dele
  shell | shell_end          Bash de fundo com "Command running in background with ID: sh-1" / fim por task:sh-1
  group                      três Read com um Agent rodando no meio (Agent em grupo de 3+) e o fim dele em group_end
  group_end                  fim do Agent do grupo (task:ag-grp)
  clear                      /clear: `reset` no SSE e a conversa volta ao começo
  long                       30 respostas a mais, para a conversa rolar
  calm                       as outras sessões que trabalham (sintetica-parser) ficam ociosas
GET /control/t15reset volta tudo ao começo. Sessões sintetica-extra-1…4 (ociosas) só alongam a barra lateral.
"""
import pathlib
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task22_perf_fixture.py").read_text(encoding="utf-8")
T22NS = {"__name__": "task22_base", "__file__": str(HERE / "task22_perf_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task22_perf_fixture.py", "exec"), T22NS)

BASE = T22NS["BASE"]
LOCK, SESSIONS, bump, info, state, msg, call = (BASE[k] for k in ("LOCK", "SESSIONS", "bump", "info", "state", "msg", "call"))

NAME = "sintetica-agentes"
CWD = "/sintetica/projetos/hangar-sintetico"


def result(eid, tid, text):
    return {"kind": "tool_result", "id": eid, "text": None, "tool_use_id": tid, "result": text, "is_error": False}


def agent(eid, tid, description, prompt, kind="Agent", **extra):
    return call(eid, tid, kind, {"description": description, "subagent_type": "Explore", "model": "haiku", "prompt": prompt, **extra})


START = [
    msg("user_msg", "a0", "Revisa o fold de atividade e roda os agentes."),
    msg("assistant_msg", "a1", "Vou dividir em subagentes: um lê o web, outro confere o nativo."),
]


def launch(eid, tid, agent_id):
    return result(eid, tid, f"Async agent launched successfully.\nagentId: {agent_id} (internal ID - do not mention to user)\n"
                            "The agent is working in the background.")


def note(eid, task):
    return msg("user_msg", eid, f"<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n"
                                "<summary>Agente sintético terminou</summary>\n</task-notification>")


STEPS = {
    "fg": [agent("live-fg", "t-fg", "Ler o fold do web", "Leia packages/core/src/activity.ts (sintético).")],
    "fg_end": [result("live-fg-r", "t-fg", "Leitura sintética concluída: 12 casos.")],
    "bg": [agent("live-bg", "t-bg", "Conferir o nativo em segundo plano", "Confira conversation.rs (sintético).", run_in_background=True),
           launch("live-bg-r", "t-bg", "ag-bg1")],
    "bg_task": [result("live-bg-end", "task:ag-bg1", "")],
    "bg2": [agent("live-bg2", "t-bg2", "Medir quadros em segundo plano", "Meça os quadros (sintético).", run_in_background=True)],
    "bg2_note": [note("live-bg2-note", "ag-bg2")],
    "bg2_launch": [launch("live-bg2-r", "t-bg2", "ag-bg2")],
    "swarm": [agent("live-sw", "t-sw", "Revisar as quatro telas", "Revise cada tela (sintético).", kind="AgentSwarm", items=["a", "b", "c"])],
    "swarm_end": [result("live-sw-r", "t-sw", "Três revisões sintéticas prontas.")],
    "shell": [call("live-sh", "t-sh", "Bash", {"command": "cd /sintetica/x && cargo build 2>&1 | tail -5", "run_in_background": True}),
              result("live-sh-r", "t-sh", "Command running in background with ID: sh-1")],
    "shell_end": [result("live-sh-end", "task:sh-1", "")],
    "group": [call("live-g1", "t-g1", "Read", {"file_path": "/sintetica/a.rs"}),
              agent("live-g2", "t-g2", "Agente no meio do grupo", "Leia b.rs (sintético).", run_in_background=True),
              launch("live-g2-r", "t-g2", "ag-grp"),
              call("live-g3", "t-g3", "Read", {"file_path": "/sintetica/c.rs"}),
              call("live-g4", "t-g4", "Read", {"file_path": "/sintetica/d.rs"})],
    "group_end": [result("live-g2-end", "task:ag-grp", "")],
}

LIVE = {"thinking": "", "tool": "", "preview": "", "label": False, "label_from": 0.0, "epoch": 0, "version": 0}


def scene_reset():
    data = info(NAME, "claude", state="idle", branch="main")
    data["cwd"] = CWD
    SESSIONS[NAME] = {"info": data, "state": state("idle"), "events": [dict(e) for e in START], "stats": None, "modes": []}
    for i in range(1, 5):
        extra = info(f"sintetica-extra-{i}", "claude", state="idle", branch="main")
        extra["cwd"] = f"/sintetica/estudos/extra-{i}"
        SESSIONS[extra["name"]] = {"info": extra, "state": state("idle"), "events": [msg("assistant_msg", f"x{i}", "Sessão extra.")],
                                   "stats": None, "modes": []}
    LIVE.update(thinking="", tool="", preview="", label=False, label_from=0.0, version=LIVE["version"] + 1)
    bump()


def set_state(value):
    SESSIONS[NAME]["state"] = state(value)
    SESSIONS[NAME]["info"]["state"] = value
    bump()


BASE_FILL = T22NS["fill"]


def fill():
    BASE_FILL()
    scene_reset()


T22NS["fill"] = fill
scene_reset()


def apply(step):
    s = SESSIONS[NAME]
    if step in STEPS:
        s["events"].extend(dict(e) for e in STEPS[step])
    elif step in ("working", "idle"):
        LIVE["label"] = False
        set_state(step)
    elif step == "label":
        LIVE.update(label=True, label_from=time.time())
        set_state("working")
    elif step in ("thinking", "nothinking"):
        LIVE["thinking"] = "Conferindo se o lançamento em segundo plano fecha o par… (sintético)" if step == "thinking" else ""
    elif step in ("tool", "notool"):
        LIVE["tool"] = '{"nome": "Grep", "input": {"pattern": "agentId", "path": "src"}}' if step == "tool" else ""
    elif step in ("preview", "nopreview"):
        LIVE["preview"] = "Os dois agentes terminaram; o de fundo fechou pelo **task:** sintético." if step == "preview" else ""
    elif step == "long":
        # Conversa longa antes do fim, para a linha de trabalhando poder sair da tela ao rolar.
        n = len(s["events"])
        s["events"].extend(msg("assistant_msg", f"live-long-{n}-{i}", f"Parágrafo sintético {i} para alongar a conversa. " * 6)
                           for i in range(30))
    elif step == "calm":
        # As outras sessões trabalhando (sintetica-parser, da Task 14) param: a medição de zero quadros não as conta.
        for other, data in SESSIONS.items():
            if other != NAME and data["info"]["state"] == "working":
                data["state"], data["info"]["state"] = state("idle"), "idle"
    elif step == "clear":
        s["events"] = [msg("user_msg", "c0", "/clear"), msg("assistant_msg", "c1", "Conversa limpa (sintético).")]
        LIVE["epoch"] += 1
    else:
        return False
    LIVE["version"] += 1
    bump()
    return True


class Handler(T22NS["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t15":
            steps = parse_qs(url.query).get("do", [""])[0].split(",")
            with LOCK:
                done = [step for step in steps if apply(step)]
            self.send_json({"done": done, "events": len(SESSIONS[NAME]["events"])})
            return
        if url.path == "/control/t15reset":
            with LOCK:
                scene_reset()
                LIVE["epoch"] += 1
            self.send_json({"ok": True})
            return
        super().do_GET()

    def stream_session(self, name):
        if name != NAME:
            return super().stream_session(name)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        sent, version, epoch = set(), -1, None
        shown = {"thinking": None, "tool": None, "preview": None}
        last_label = None
        try:
            while True:
                frames = []
                with LOCK:
                    s = SESSIONS[NAME]
                    if epoch is not None and LIVE["epoch"] != epoch:
                        # /clear: o backend manda `reset` e o app relê o histórico.
                        frames.append(("reset", {}, None))
                        sent = {e["id"] for e in s["events"]}
                    epoch = LIVE["epoch"]
                    label = None
                    if LIVE["label"]:
                        label = f"Computing… ({int(time.time() - LIVE['label_from']) + 1}s)"
                    if LIVE["version"] != version or label != last_label:
                        version, last_label = LIVE["version"], label
                        frames.append(("state", {"session": NAME, **s["state"], "label": label}, None))
                    for event in s["events"]:
                        if event["id"].startswith("live-") and event["id"] not in sent:
                            sent.add(event["id"])
                            frames.append(("message", event, f"fixture:{event['id']}"))
                    for slot, kind in (("thinking", "pensamento"), ("tool", "ferramenta")):
                        if shown[slot] != LIVE[slot]:
                            shown[slot] = LIVE[slot]
                            frames.append((kind, {"text": LIVE[slot]}, None))
                    if shown["preview"] != LIVE["preview"]:
                        shown["preview"] = LIVE["preview"]
                        frames.append(("preview", {"text": LIVE["preview"], "md": True, "full": True, "vivo": True}, None))
                for kind, body, eid in frames or [("ping", {}, None)]:
                    self.frame(kind, body, eid)
                time.sleep(0.2)
        except (BrokenPipeError, ConnectionResetError):
            pass


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
