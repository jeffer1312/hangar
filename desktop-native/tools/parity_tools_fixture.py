"""Synthetic backend for the native chat Task 2 checks: tools, thinking and reading.

Nothing here talks to a real session. /control/start plays the live turn; /control/more
appends events while the reader sits higher up in the list.
"""

import json
import os
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

TOKEN = "parity-tools-fixture"
MAIN = "synthetic-tools"
OTHER = "synthetic-other"
START = threading.Event()
MORE = threading.Event()
R2 = threading.Event()

LONG_RESULT = "\n".join(f"{i:05d} linha longa do resultado com texto suficiente para ocupar a largura inteira" for i in range(1, 1201))
CODE_ANSWER = """Resumo da leitura:

- A lista **mantém** a posição de leitura.
- Ferramentas ficam recolhidas por padrão.

```rust
fn main() {
    println!("código com `crases` e acentuação: ação");
}
```

Parágrafo longo para conferir a largura da prosa. """ + " ".join(["A coluna de leitura deve parar numa largura confortável em vez de ocupar a janela inteira."] * 4)


def session(name, state):
    return {"name": name, "cwd": f"/synthetic/{name}", "jsonl": f"{name}.jsonl", "provider": "claude", "state": state, "tracked": True}


def call(eid, tid, name, tool_input):
    return {"kind": "tool_use", "id": eid, "tool_use_id": tid, "tool_name": name, "tool_input": tool_input}


def result(eid, tid, text, error=False):
    return {"kind": "tool_result", "id": eid, "tool_use_id": tid, "result": text, "is_error": error}


def history():
    events = []
    # 400+ eventos: o cliente oferece "Mais antigas", e o clique reaplica o histórico por cima do vivo.
    for i in range(1, 191):
        events.append({"kind": "user_msg", "id": f"fill-u{i}", "text": f"Pergunta antiga {i}."})
        events.append({"kind": "assistant_msg", "id": f"fill-a{i}", "text": f"Resposta antiga {i}, só para haver rolagem."})
    events += [
        {"kind": "user_msg", "id": "u1", "text": "Investigue o módulo e mostre o que encontrou."},
        {"kind": "thinking", "id": "t1", "text": "Preciso primeiro entender a estrutura. Depois procuro a documentação oficial.\n\nSegundo parágrafo do raciocínio."},
        call("evt-search", "toolu_search", "WebSearch", {"query": "gpui list remeasure items"}),
        result("evt-search-r", "toolu_search", "3 resultados encontrados"),
        {"kind": "thinking", "id": "t2", "text": "A busca confirmou o comportamento. Agora leio o arquivo."},
        call("evt-read", "toolu_read", "Read", {"file_path": "/synthetic/src/app.rs", "offset": 10, "limit": 40}),
        result("evt-read-r", "toolu_read", "fn main() {}\nstruct App;\n"),
        call("evt-bash-err", "toolu_bash_err", "Bash", {"command": "cargo build --locked"}),
        result("evt-bash-err-r", "toolu_bash_err", "error[E0425]: cannot find value `x` in this scope\n --> src/main.rs:3:5", True),
        {"kind": "assistant_msg", "id": "a1", "text": "O build falhou; vou olhar mais arquivos."},
        call("g1", "toolu_g1", "Read", {"file_path": "/synthetic/src/one.rs"}),
        result("g1-r", "toolu_g1", "um\ndois"),
        call("g2", "toolu_g2", "Grep", {"pattern": "remeasure", "path": "src"}),
        result("g2-r", "toolu_g2", "src/app.rs:10: remeasure"),
        call("g3", "toolu_g3", "Bash", {"command": "cat /synthetic/huge.log"}),
        result("g3-r", "toolu_g3", LONG_RESULT),
        call("g4", "toolu_g4", "Bash", {"command": "false"}),
        result("g4-r", "toolu_g4", "exit status 1", True),
        {"kind": "assistant_msg", "id": "a2", "text": CODE_ANSWER},
        call("dup-a", "toolu_dup", "Read", {"file_path": "/synthetic/a.rs"}),
        call("dup-b", "toolu_dup", "Read", {"file_path": "/synthetic/b.rs"}),
        result("dup-a-r", "toolu_dup", "conteúdo de a"),
        result("dup-b-r", "toolu_dup", "conteúdo de b"),
        result("orphan", "toolu_gone", "saída de uma chamada que ficou fora da janela"),
        call("no-result", "toolu_none", "Bash", {"command": "sleep 999"}),
        {"kind": "assistant_msg", "id": "rep-intro", "text": "Resultado antigo e chamada nova com o mesmo id:"},
        result("rep-old", "toolu_rep", "saída antiga, anterior à chamada"),
        call("rep-new", "toolu_rep", "Bash", {"command": "echo repetido"}),
        result("rep-new-r", "toolu_rep", "saída da chamada nova"),
        {"kind": "assistant_msg", "id": "a3", "text": "Fim do histórico."},
    ]
    return events


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def send_json(self, body):
        data = json.dumps(body, ensure_ascii=False).encode()
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
        print(f"{time.monotonic():.3f} {kind}", flush=True)

    def round_two(self):
        thought = "Pensamento B em andamento"
        tool_b = {"nome": "Bash", "input": {"command": "echo B"}}
        self.frame("state", {"state": "working"})
        self.frame("pensamento", {"text": thought})
        self.frame("ferramenta", {"text": json.dumps(tool_b)})
        time.sleep(1)
        # Replay de itens A já conhecidos: não pode apagar B.
        self.frame("message", {"kind": "thinking", "id": "t1", "text": "Preciso primeiro entender a estrutura. Depois procuro a documentação oficial.\n\nSegundo parágrafo do raciocínio."}, "fixture:r2a")
        self.frame("message", call("evt-read", "toolu_read", "Read", {"file_path": "/synthetic/src/app.rs", "offset": 10, "limit": 40}), "fixture:r2b")
        time.sleep(8)
        self.frame("message", {"kind": "thinking", "id": "live-tb", "text": thought + ", concluído."}, "fixture:r2c")
        time.sleep(0.3)
        self.frame("pensamento", {"text": thought})
        time.sleep(1.5)
        self.frame("message", call("live-callb", "toolu_b", "Bash", {"command": "echo B"}), "fixture:r2d")
        time.sleep(0.3)
        self.frame("ferramenta", {"text": json.dumps(tool_b)})
        time.sleep(1.5)
        self.frame("message", result("live-resb", "toolu_b", "B"), "fixture:r2e")
        self.frame("pensamento", {"text": ""})
        self.frame("ferramenta", {"text": ""})
        self.frame("state", {"state": "idle"})

    def idle(self):
        while True:
            if R2.is_set():
                R2.clear()
                self.round_two()
            if MORE.is_set():
                MORE.clear()
                stamp = int(time.time() * 1000)
                self.frame("state", {"state": "working"})
                self.frame("message", {"kind": "assistant_msg", "id": f"more-{stamp}", "text": f"Evento novo {stamp} chegou enquanto você lia acima."}, f"fixture:m{stamp}")
                self.frame("message", call(f"more-call-{stamp}", f"toolu_more_{stamp}", "Bash", {"command": "echo novo"}), f"fixture:c{stamp}")
                self.frame("message", result(f"more-res-{stamp}", f"toolu_more_{stamp}", "novo"), f"fixture:r{stamp}")
                self.frame("state", {"state": "idle"})
            self.frame("ping", {})
            time.sleep(0.5)

    def live_turn(self):
        self.frame("state", {"state": "working"})
        thought = ""
        for piece in ["Vou conferir", " o arquivo de configuração", " antes de responder."]:
            thought += piece
            self.frame("pensamento", {"text": thought})
            time.sleep(0.8)
        self.frame("message", {"kind": "thinking", "id": "live-t", "text": thought}, "fixture:1")
        self.frame("pensamento", {"text": ""})
        self.frame("ferramenta", {"text": json.dumps({"nome": "Bash", "input": {"command": "ls -la /synthetic"}})})
        time.sleep(2.5)
        self.frame("message", call("live-call", "toolu_live", "Bash", {"command": "ls -la /synthetic"}), "fixture:2")
        self.frame("ferramenta", {"text": ""})
        time.sleep(1.5)
        self.frame("message", result("live-res", "toolu_live", "total 0\ndrwxr-xr-x synthetic"), "fixture:3")
        text = ""
        for piece in ["A pasta", " tem só", " um diretório."]:
            text += piece
            self.frame("preview", {"text": text, "md": True, "full": True, "vivo": True})
            time.sleep(0.6)
        self.frame("message", {"kind": "assistant_msg", "id": "live-a", "text": text}, "fixture:4")
        self.frame("preview", {"text": "", "md": True, "full": True, "vivo": True})
        self.frame("state", {"state": "idle"})

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/control/start":
            START.set()
            self.send_json({"started": True})
            return
        if path == "/control/r2":
            R2.set()
            self.send_json({"r2": True})
            return
        if path == "/control/more":
            MORE.set()
            self.send_json({"more": True})
            return
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.send_response(401)
            self.end_headers()
            return
        sessions = [session(MAIN, "idle"), session(OTHER, "idle")]
        if path == "/api/sessions":
            self.send_json(sessions)
        elif path == "/api/sessions/events":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            try:
                self.frame("sessions", sessions)
                while True:
                    self.frame("ping", {})
                    time.sleep(1)
            except (BrokenPipeError, ConnectionResetError):
                pass
        elif path == f"/api/sessions/{MAIN}/history":
            self.send_json(history())
        elif path == f"/api/sessions/{OTHER}/history":
            self.send_json([{"kind": "user_msg", "id": "o1", "text": "Outra sessão."}, {"kind": "assistant_msg", "id": "o2", "text": "Nada de ferramentas aqui."}])
        elif path in (f"/api/sessions/{MAIN}/events", f"/api/sessions/{OTHER}/events"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            try:
                self.frame("state", {"state": "working"})
                if path.endswith(f"{MAIN}/events"):
                    while not START.wait(0.3):
                        self.frame("ping", {})
                    START.clear()
                    self.live_turn()
                else:
                    self.frame("state", {"state": "idle"})
                self.idle()
            except (BrokenPipeError, ConnectionResetError):
                pass
        else:
            self.send_response(404)
            self.end_headers()


server = ThreadingHTTPServer(("127.0.0.1", int(os.environ.get("PARITY_TOOLS_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port} token={TOKEN}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
