"""Fixture SINTÉTICA da Task 22 (suavidade): a da Task 14 (barra, diálogo de criar, menu da sessão) com duas sessões a mais,
para o roteiro de medição do tempo de quadro. Nenhuma sessão, pasta ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 14 sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.

Sessões daqui:
  sintetica-longa   conversa longa (a de ferramentas da Task 12 R3a repetida 40 vezes), para rolar.
  sintetica-stream  resposta longa em streaming, disparada por GET /control/t22stream (cada disparo é uma resposta nova,
                    com pedaços de ~120 ms, como a prévia real; no fim vira mensagem gravada e a sessão volta a ociosa).
GET /control/t22status diz quantas respostas já foram disparadas e terminadas.
Os modos da Task 13 (/control/t13, ex.: scan_delay=2 para a lista de pastas carregando) e da Task 14 continuam valendo.
"""
import pathlib
import threading
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task14_sidebar_fixture.py").read_text(encoding="utf-8")
T14NS = {"__name__": "task14_base", "__file__": str(HERE / "task14_sidebar_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task14_sidebar_fixture.py", "exec"), T14NS)

BASE = T14NS["BASE"]
LOCK, SESSIONS, bump, info, state, msg = (BASE[k] for k in ("LOCK", "SESSIONS", "bump", "info", "state", "msg"))

LONG_NAME, STREAM_NAME = "sintetica-longa", "sintetica-stream"
PARAGRAPH = ("Quando a resposta cresce, a conversa precisa descer junto sem saltos. O olho acompanha melhor um movimento "
             "contínuo do que uma sequência de pulos: ação, função, informação e coração aparecem para que nenhum corte "
             "de texto se esconda no meio de uma letra.")
ANSWER = "\n\n".join(
    [f"## Parte {i}\n\n{PARAGRAPH}\n\n- Item um da parte {i}, com **negrito** e `código`.\n- Item dois da parte {i}, mais longo, "
     f"para ocupar duas linhas quando a coluna da conversa fica apertada entre as barras." for i in range(1, 9)]
    + ["```rust\nlet next = spring.step(pos, target, frames);\n```", PARAGRAPH])
OLD = [msg(kind, f"s-{i}-{kind}", f"Pergunta {i}: como a rolagem se comporta?" if kind == "user_msg" else PARAGRAPH)
       for i in range(1, 7) for kind in ("user_msg", "assistant_msg")]

RUN = {"started": 0, "done": 0}
FLIP = {"until": 0.0}
TRIGGER = threading.Condition()


def add_rows():
    data = info(LONG_NAME, "claude", state="idle", branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    SESSIONS[LONG_NAME] = {"info": data, "state": state("idle"), "events": BASE["long_events"](40), "stats": None, "modes": []}
    data = info(STREAM_NAME, "claude", state="idle", branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    SESSIONS[STREAM_NAME] = {"info": data, "state": state("idle"), "events": [dict(e) for e in OLD], "stats": None, "modes": []}
    bump()


BASE_FILL = T14NS["fill"]


def fill():
    BASE_FILL()
    add_rows()


T14NS["fill"] = fill
add_rows()


class Handler(T14NS["Handler"]):
    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/control/t22stream":
            with TRIGGER:
                RUN["started"] += 1
                TRIGGER.notify_all()
            self.send_json(dict(RUN))
            return
        if path == "/control/t22status":
            self.send_json(dict(RUN))
            return
        if path == "/control/t22state":
            # Estado da sintetica-longa alternando a cada 1 s por `secs` segundos, só quadros `state` no SSE.
            # pending=1 acrescenta antes uma chamada sem resultado no fim, para o "em execução" aparecer.
            query = parse_qs(urlparse(self.path).query)
            with LOCK:
                if query.get("pending") == ["1"] and not any(e["id"] == "live-t22-pending" for e in SESSIONS[LONG_NAME]["events"]):
                    SESSIONS[LONG_NAME]["events"].append(BASE["call"]("live-t22-pending", "t22p", "Bash", {"command": "cargo build --release"}))
                FLIP["until"] = time.time() + float(query.get("secs", ["10"])[0])
            self.send_json({"until": FLIP["until"]})
            return
        super().do_GET()

    def do_POST(self):
        # O envio responde depois de 1,5 s: dá tempo de ver na tela o "Enviando…" que o Enter mostra.
        if urlparse(self.path).path.endswith("/input"):
            time.sleep(1.5)
        super().do_POST()

    def long_stream(self, name):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        sent, working = set(), False
        try:
            with LOCK:
                self.frame("state", {"session": name, **SESSIONS[name]["state"]})
                self.frame("ask_question", None)
            last = time.time()
            while True:
                with LOCK:
                    frames = [("message", e, f"fixture:{e['id']}") for e in SESSIONS[name]["events"]
                              if e["id"].startswith("live-") and e["id"] not in sent]
                    sent.update(body["id"] for _, body, _ in frames)
                    flipping = time.time() < FLIP["until"]
                if flipping and time.time() - last >= 1.0:
                    working, last = not working, time.time()
                    frames.append(("state", {"session": name, **state("working" if working else "idle")}, None))
                elif not flipping and working:
                    working = False
                    frames.append(("state", {"session": name, **state("idle")}, None))
                for kind, body, eid in frames or [("ping", {}, None)]:
                    self.frame(kind, body, eid)
                time.sleep(0.3)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def stream_session(self, name):
        if name == LONG_NAME:
            return self.long_stream(name)
        if name != STREAM_NAME:
            return super().stream_session(name)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        try:
            with LOCK:
                self.frame("state", {"session": name, **SESSIONS[name]["state"]})
            with TRIGGER:
                seen = RUN["started"]
            while True:
                with TRIGGER:
                    TRIGGER.wait(1.0)
                    run = RUN["started"]
                if run == seen:
                    self.frame("ping", {})
                    continue
                seen = run
                self.play(name, run)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def play(self, name, run):
        """Uma resposta longa crescendo em pedaços de duas palavras a cada ~120 ms; no fim, mensagem gravada."""
        with LOCK:
            SESSIONS[name]["state"] = state("working")
            SESSIONS[name]["info"]["state"] = "working"
            bump()
            self.frame("state", {"session": name, **SESSIONS[name]["state"]})
        words, text = ANSWER.split(" "), ""
        for i in range(0, len(words), 2):
            text += ("" if not text else " ") + " ".join(words[i:i + 2])
            # vivo=False: prévia lida do pane (Claude com terminal), a que a janela anima quadro a quadro.
            self.frame("preview", {"text": text, "md": True, "full": True, "vivo": False})
            time.sleep(0.12)
        final = msg("assistant_msg", f"live-t22-{run}", text)
        with LOCK:
            SESSIONS[name]["events"].append(final)
            SESSIONS[name]["state"] = state("idle")
            SESSIONS[name]["info"]["state"] = "idle"
            RUN["done"] += 1
            bump()
        self.frame("message", final, f"fixture:t22-{run}")
        self.frame("preview", {"text": "", "md": False, "full": True, "vivo": False})
        self.frame("state", {"session": name, **state("idle")})


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
