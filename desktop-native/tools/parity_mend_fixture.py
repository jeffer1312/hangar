"""Fixture isolada da Task 10: prévia com marcação aberta no meio do pedaço e esmaecimento do texto novo.

PARITY_MEND_MODE=steps (padrão): cada GET /control/next manda um pedaço que termina com uma marcação
aberta; o seguinte a fecha. Serve para capturar a linha antes e depois do fechamento. Depois do último,
a mensagem final chega com o texto idêntico ao recebido.
PARITY_MEND_MODE=fade: GET /control/start solta uma resposta longa em pedaços de prosa (vivo falso,
então o app dosa a entrada) sobre uma conversa comprida, para ver o esmaecimento e o seguir-o-fim.
Só escuta em 127.0.0.1, porta de PARITY_MEND_PORT (0 = aleatória).
"""

import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

TOKEN = "parity-mend-fixture"
SESSION = "synthetic-mend"
MODE = os.environ.get("PARITY_MEND_MODE", "steps")
INTRO = ("Esta resposta é longa de propósito para quebrar em várias linhas na coluna da conversa, "
         "e cada marcação abre no fim de um pedaço e só fecha no pedaço seguinte: ")
STEPS = [
    INTRO + "primeiro um **negr",
    INTRO + "primeiro um **negrito comprido o bastante** e depois `cargo build --rele",
    INTRO + "primeiro um **negrito comprido o bastante** e depois `cargo build --release` e ~~um trecho ris",
    INTRO + "primeiro um **negrito comprido o bastante** e depois `cargo build --release` e ~~um trecho riscado~~ "
            "e [a documentação do kit](https://gpui-kit.com/do",
    INTRO + "primeiro um **negrito comprido o bastante** e depois `cargo build --release` e ~~um trecho riscado~~ "
            "e [a documentação do kit](https://gpui-kit.com/docs) e *uma ênfase no fi",
    INTRO + "primeiro um **negrito comprido o bastante** e depois `cargo build --release` e ~~um trecho riscado~~ "
            "e [a documentação do kit](https://gpui-kit.com/docs) e *uma ênfase no fim*.",
]
OLD = ("Resposta {i}. Mensagem antiga para existir histórico acima do fim, com acentuação: ação, função e coração.\n\n"
       "- Item com **negrito** e `código`.\n- Item mais longo para ocupar duas linhas quando a coluna aperta.")
FADE_ANSWER = " ".join(
    ["O texto novo entra esmaecendo enquanto a resposta chega, e o que já estava na tela fica parado."] * 3
    + ["\n\nO segundo parágrafo continua a resposta para que ela passe da altura da janela e a conversa"
       " precise seguir o fim enquanto o texto ainda cresce, linha após linha, até a última palavra."] * 4
)
NEXT = threading.Condition()
STEP = 0
START = threading.Event()


def session():
    return {"name": SESSION, "cwd": "/synthetic/mend", "jsonl": "synthetic-mend.jsonl",
            "provider": "codex", "state": "working", "tracked": True}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def send_json(self, body):
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def frame(self, kind, body, event_id=None):
        payload = f"event: {kind}\ndata: {json.dumps(body, ensure_ascii=False)}\n"
        if event_id:
            payload += f"id: {event_id}\n"
        self.wfile.write((payload + "\n").encode())
        self.wfile.flush()
        if kind in ("preview", "message", "state"):
            print(f"{time.monotonic():.3f} {kind} chars={len(body.get('text', ''))}", flush=True)

    def idle_forever(self):
        self.frame("state", {"state": "idle"})
        while True:
            self.frame("ping", {})
            time.sleep(1)

    def steps(self):
        global STEP
        sent = 0
        while True:
            with NEXT:
                NEXT.wait_for(lambda: STEP > sent, timeout=1)
                target = STEP
            if target == sent:
                self.frame("ping", {})
                continue
            sent = target
            if sent <= len(STEPS):
                self.frame("preview", {"text": STEPS[sent - 1], "md": True, "full": True, "vivo": True})
            else:
                self.frame("message", {"kind": "assistant_msg", "id": "final", "text": STEPS[-1]}, "fixture:10")
                self.frame("preview", {"text": "", "md": False, "full": True, "vivo": False})
                self.idle_forever()

    def fade(self):
        while not START.wait(0.5):
            self.frame("ping", {})
        words = FADE_ANSWER.split(" ")
        text = ""
        for i in range(0, len(words), 4):
            text += ("" if not text else " ") + " ".join(words[i:i + 4])
            self.frame("preview", {"text": text, "md": True, "full": True, "vivo": False})
            time.sleep(0.4)
        time.sleep(1.5)
        self.frame("message", {"kind": "assistant_msg", "id": "final", "text": text}, "fixture:10")
        self.frame("preview", {"text": "", "md": False, "full": True, "vivo": False})
        self.idle_forever()

    def do_GET(self):
        global STEP
        path = urlparse(self.path).path
        if path == "/control/next":
            with NEXT:
                STEP += 1
                NEXT.notify_all()
            self.send_json({"step": STEP, "of": len(STEPS) + 1})
            return
        if path == "/control/start":
            START.set()
            self.send_json({"started": True})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.send_response(401)
            self.end_headers()
            return
        if path == "/api/sessions":
            self.send_json([session()])
        elif path == "/api/sessions/events":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            try:
                self.frame("sessions", [session()])
                while True:
                    self.frame("ping", {})
                    time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        elif path == f"/api/sessions/{SESSION}/history":
            if MODE == "fade":
                history = []
                for i in range(1, 11):
                    history.append({"kind": "user_msg", "id": f"q{i}", "text": f"Pergunta {i}: a conversa já é longa?"})
                    history.append({"kind": "assistant_msg", "id": f"a{i}", "text": OLD.format(i=i)})
            else:
                history = [{"kind": "user_msg", "id": "question", "text": "Responda com várias marcações."}]
            self.send_json(history)
        elif path == f"/api/sessions/{SESSION}/events":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            try:
                self.frame("state", {"state": "working"})
                self.fade() if MODE == "fade" else self.steps()
            except (BrokenPipeError, ConnectionResetError):
                pass
        else:
            self.send_response(404)
            self.end_headers()


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PARITY_MEND_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port} token={TOKEN}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
