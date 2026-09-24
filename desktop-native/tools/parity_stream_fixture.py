"""Isolated, manually triggered SSE stream for native chat observation."""

import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse


TOKEN = "parity-stream-fixture"
SESSION = "synthetic-stream"
MODE = os.environ.get("PARITY_STREAM_MODE", "slow")
SCROLL_OLD = (
    "Resposta {i}. A lista mostra esta mensagem antiga para que exista histórico acima do fim. "
    "Cada parágrafo tem acentuação, cedilha e til: ação, função, informação e coração.\n\n"
    "- Item um da resposta {i}, com **negrito** e `código`.\n- Item dois, mais longo, para ocupar duas linhas "
    "quando a janela é estreita e a coluna da conversa fica apertada entre as barras."
)
SCROLL_ANSWER = " ".join([
    "## Como a janela acompanha o texto\n\nQuando a resposta cresce, a conversa precisa descer junto sem saltos.",
    "O olho acompanha melhor um movimento contínuo do que uma sequência de pulos, e isso vale ainda mais",
    "quando cada pedaço novo chega a cada cento e vinte milissegundos.\n\nA primeira parte explica a mola:",
    "a posição persegue o fim com uma velocidade que cresce com a distância e diminui perto do alvo.",
    "\n\n- Se a pessoa rolou para cima, nada a puxa de volta.\n- O botão de ir para o fim desliza até lá.",
    "\n- A roda do mouse anda em passos suaves, não em degraus.\n\nA segunda parte trata da lista janelada:",
    "só as linhas visíveis são desenhadas, então a altura das que ficaram fora é uma estimativa até",
    "serem medidas. Por isso a mola lê a distância depois do layout de cada quadro.\n\n```rust\nlet next = spring.step(pos, target, frames);\n```",
    "\n\nA terceira parte fala de acentos: ação, função, informação, coração, pão, mãe, avó e café",
    "aparecem aqui para que a fixture não esconda um corte de texto no meio de uma letra.",
    "\n\nPor fim, a resposta termina com um parágrafo mais longo, que empurra o conteúdo para além da",
    "altura da janela e obriga a conversa a continuar descendo enquanto o texto ainda chega, linha após",
    "linha, até a última palavra aparecer e a sessão voltar ao estado ocioso.",
])
START = threading.Event()
CONNECTED = threading.Event()


def session():
    return {
        "name": SESSION,
        "cwd": "/synthetic/stream",
        "jsonl": "synthetic-stream.jsonl",
        "provider": "codex",
        "state": "working",
        "tracked": True,
    }


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

    def stream(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

    def frame(self, kind, body, event_id=None):
        payload = f"event: {kind}\ndata: {json.dumps(body, ensure_ascii=False)}\n"
        if event_id:
            payload += f"id: {event_id}\n"
        self.wfile.write((payload + "\n").encode())
        self.wfile.flush()
        if kind in ("preview", "message", "state"):
            print(f"{time.monotonic():.3f} {kind} chars={len(body.get('text', ''))} state={body.get('state', '')}", flush=True)

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/control/start":
            START.set()
            self.send_json({"started": True, "connected": CONNECTED.is_set()})
            return
        if path == "/control/status":
            self.send_json({"started": START.is_set(), "connected": CONNECTED.is_set()})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.send_response(401)
            self.end_headers()
            return
        if path == "/api/sessions":
            self.send_json([session()])
        elif path == "/api/sessions/events":
            self.stream()
            try:
                self.frame("sessions", [session()])
                while True:
                    self.frame("ping", {})
                    time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        elif path == f"/api/sessions/{SESSION}/history":
            history = [
                {"kind": "user_msg", "id": "question", "text": "Mostre o texto enquanto ele cresce."},
                {"kind": "assistant_msg", "id": "older", "text": "Mensagem anterior para conferir a rolagem."},
            ]
            if MODE == "codex-two":
                history[-1]["ts"] = 1000.0
            if MODE in ("scroll", "scroll-long"):
                history = []
                for i in range(1, 13):
                    history.append({"kind": "user_msg", "id": f"q{i}", "text": f"Pergunta {i}: como a rolagem se comporta quando a conversa já é longa?"})
                    history.append({"kind": "assistant_msg", "id": f"a{i}", "text": SCROLL_OLD.format(i=i)})
            self.send_json(history)
        elif path == f"/api/sessions/{SESSION}/events":
            self.stream()
            CONNECTED.set()
            try:
                self.frame("state", {"state": "working"})
                while not START.wait(0.2):
                    self.frame("ping", {})
                if MODE == "codex-two":
                    preamble = "Vou descrever a viagem."
                    answer = "Primeira linha do trajeto.\nSegunda linha da paisagem.\nTerceira linha encerra a viagem."
                    self.frame("preview", {"text": preamble, "md": True, "full": True, "vivo": True})
                    time.sleep(1)
                    self.frame("preview", {"text": "", "md": True, "full": True, "vivo": True})
                    time.sleep(0.3)
                    self.frame("preview", {"text": "Primeira linha do trajeto.", "md": True, "full": True, "vivo": True})
                    time.sleep(0.3)
                    self.frame("message", {"kind": "assistant_msg", "id": "preamble", "text": preamble, "ts": 1001.0}, "fixture:10")
                    time.sleep(1)
                    self.frame("preview", {"text": answer, "md": True, "full": True, "vivo": True})
                    time.sleep(1)
                    self.frame("preview", {"text": "", "md": True, "full": True, "vivo": True})
                    time.sleep(0.3)
                    self.frame("message", {"kind": "assistant_msg", "id": "answer", "text": answer, "ts": 1002.0}, "fixture:20")
                    self.frame("state", {"state": "idle"})
                    while True:
                        self.frame("ping", {})
                        time.sleep(1)
                if MODE in ("blocks", "blocks-no-final"):
                    chunks = [
                        "A janela mostra a primeira frase com acentuação.",
                        " O segundo bloco chega inteiro, mas aparece aos poucos na tela.",
                        " O terceiro bloco contém café, ação e uma estrela ✨ no fim.",
                    ]
                    text = ""
                    for chunk in chunks:
                        text += chunk
                        self.frame("preview", {"text": text, "md": True, "full": True, "vivo": False})
                        time.sleep(1.3)
                    if MODE == "blocks-no-final":
                        self.frame("state", {"state": "idle"})
                        while True:
                            self.frame("ping", {})
                            time.sleep(1)
                    text += " Esta frase usa vivo verdadeiro e deve aparecer de uma vez."
                    self.frame("preview", {"text": text, "md": True, "full": True, "vivo": True})
                    time.sleep(1.2)
                    self.frame("message", {"kind": "assistant_msg", "id": "final", "text": text}, "fixture:10")
                    self.frame("state", {"state": "idle"})
                    while True:
                        self.frame("ping", {})
                        time.sleep(1)
                if MODE == "replay":
                    # O pane reemite a última prosa depois que ela já foi gravada e uma ferramenta começou.
                    said = "Uso o despachante Lua do Hyprland para aumentar a janela. É só um clique na sessão."
                    self.frame("preview", {"text": said, "md": False, "full": False, "vivo": False})
                    time.sleep(1)
                    self.frame("message", {"kind": "assistant_msg", "id": "said", "text": said}, "fixture:10")
                    time.sleep(0.5)
                    self.frame("message", {"kind": "tool_use", "id": "call", "tool_name": "Bash",
                                           "tool_input": {"command": "hyprctl monitors -j"}, "tool_use_id": "t1"}, "fixture:11")
                    time.sleep(1)
                    self.frame("preview", {"text": said, "md": False, "full": False, "vivo": False})
                    while True:
                        self.frame("ping", {})
                        time.sleep(1)
                if MODE in ("scroll", "scroll-long"):
                    answer = SCROLL_ANSWER
                    if MODE == "scroll-long":
                        # Pensamento e ferramenta ao vivo antes; resposta com mais de duas alturas de janela.
                        thought = ""
                        for piece in ["Vou medir a rolagem", " com uma resposta longa", " antes de responder."]:
                            thought += piece
                            self.frame("pensamento", {"text": thought})
                            time.sleep(0.5)
                        self.frame("message", {"kind": "thinking", "id": "live-t", "text": thought}, "fixture:1")
                        self.frame("pensamento", {"text": ""})
                        self.frame("ferramenta", {"text": json.dumps({"nome": "Bash", "input": {"command": "hyprctl monitors -j"}})})
                        time.sleep(1.5)
                        self.frame("message", {"kind": "tool_use", "id": "call", "tool_use_id": "t1", "tool_name": "Bash",
                                               "tool_input": {"command": "hyprctl monitors -j"}}, "fixture:2")
                        time.sleep(0.5)
                        self.frame("message", {"kind": "tool_result", "id": "res", "tool_use_id": "t1",
                                               "result": "DP-1 1920x1080\nHDMI-A-1 1920x1080", "is_error": False}, "fixture:3")
                        self.frame("ferramenta", {"text": ""})
                        answer = "\n\n".join([SCROLL_ANSWER] * 3)
                    # Resposta longa crescendo em pedaços curtos, no ritmo da prévia real (~120 ms).
                    words = answer.split(" ")
                    text = ""
                    for i in range(0, len(words), 2):
                        text += ("" if not text else " ") + " ".join(words[i:i + 2])
                        self.frame("preview", {"text": text, "md": True, "full": True, "vivo": True})
                        time.sleep(0.12)
                    self.frame("message", {"kind": "assistant_msg", "id": "final", "text": text}, "fixture:10")
                    self.frame("preview", {"text": "", "md": False, "full": True, "vivo": False})
                    self.frame("state", {"state": "idle"})
                    while True:
                        self.frame("ping", {})
                        time.sleep(1)
                if MODE in ("pane", "pane-no-final"):
                    text = "Linha um em leitura.\nLinha dois continua aqui.\nLinha três fecha o bloco."
                    self.frame("preview", {"text": text, "md": False, "full": False, "vivo": False})
                    time.sleep(1)
                    self.frame("preview", {"text": text[:20], "md": False, "full": False, "vivo": False})
                    time.sleep(0.5)
                    self.frame("preview", {"text": text, "md": False, "full": False, "vivo": False})
                    time.sleep(0.5)
                    self.frame("preview", {"text": "", "md": False, "full": False, "vivo": False})
                    time.sleep(0.8)
                    self.frame("preview", {"text": text, "md": False, "full": False, "vivo": False})
                    time.sleep(0.8)
                    self.frame("state", {"state": "idle"})
                    time.sleep(1.2)
                    if MODE == "pane":
                        self.frame("message", {"kind": "assistant_msg", "id": "final", "text": text}, "fixture:10")
                    while True:
                        self.frame("ping", {})
                        time.sleep(1)
                parts = [
                    "Primeira linha visível.",
                    "\nSegunda linha em progresso.",
                    "\nTerceira linha completa, com acentuação e **Markdown**.",
                    "\nQuarta linha para deslocar o conteúdo sem sumir.",
                ] if MODE == "slow" else [f" Palavra {i}." for i in range(1, 81)]
                text = ""
                for part in parts:
                    text += part
                    self.frame("preview", {"text": text, "md": MODE != "slow", "full": True, "vivo": True})
                    time.sleep(1.2 if MODE == "slow" else 0.08)
                self.frame("message", {"kind": "assistant_msg", "id": "final", "text": text}, "fixture:10")
                self.frame("preview", {"text": "", "md": False, "full": True, "vivo": False})
                self.frame("state", {"state": "idle"})
                while True:
                    self.frame("ping", {})
                    time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        else:
            self.send_response(404)
            self.end_headers()


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PARITY_STREAM_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port} token={TOKEN}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
