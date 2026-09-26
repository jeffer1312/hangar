"""Fixture SINTÉTICA da Task 35 (ferramentas no visual Árvore, com o raciocínio dentro do grupo): a da Task 15d com
uma sessão a mais. Nenhuma sessão, pasta, agente ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 15d sem o bloco que sobe o servidor (o arquivo dela não muda).

Sessão sintetica-arvore, trabalhando, na ordem:
- raciocínio longo + busca na web (consulta mais longa da fixture) + Grep, tudo pronto: grupo com raciocínio;
- resposta; Read pronto + Bash com erro: grupo com falha;
- resposta; Agent concluído: fica fora do grupo, com o cartão dele;
- raciocínio + Read pronto + Bash sem resultado, depois da última mensagem: o grupo que roda, aberto sozinho.
GET /control/t35?state=working|between|idle troca o estado da sessão: working = como acima; between = trabalhando, mas
sem o Bash pendente (o turno entre uma chamada e a próxima: o último grupo continua aberto); idle = ocioso, com o Bash
pendente (o último grupo deixa de rodar e fecha).
"""
import pathlib
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task15_cards_fixture.py").read_text(encoding="utf-8")
T15NS = {"__name__": "task15_base", "__file__": str(HERE / "task15_cards_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task15_cards_fixture.py", "exec"), T15NS)

BASE = T15NS["BASE"]
SESSIONS, LOCK, bump, info, state, msg, call, result = (BASE[k] for k in ("SESSIONS", "LOCK", "bump", "info", "state", "msg", "call", "result"))

NAME = "sintetica-arvore"
NOW = time.time()
THOUGHT = ("Vou redigir o relatório em português, citando as fontes das buscas e conferindo cada número contra o "
           "artigo original antes de escrever. Depois procuro no código onde o parser lê as linhas, porque a resposta "
           "precisa apontar o arquivo e a função exatos, não só o módulo. Se a busca não trouxer a versão certa, refaço "
           "com o nome do pacote e o ano, e só então fecho o texto com a comparação que ele pediu (sintético).")
QUERY = "formato do transcript jsonl do Claude Code com eventos de raciocínio, ferramentas e resultados em 2026 (sintético)"

EVENTS = [
    msg("user_msg", "z-u1", "Pesquisa o formato do transcript e me diz onde o parser lê as linhas.", ts=NOW - 240),
    msg("thinking", "z-th1", THOUGHT),
    call("z-ws", "zws", "WebSearch", {"query": QUERY}), result("z-ws-r", "zws", "5 resultados\nformato jsonl"),
    call("z-gr", "zgr", "Grep", {"pattern": "fn parse_line", "path": "src"}), result("z-gr-r", "zgr", "src/parser.rs:12"),
    msg("assistant_msg", "z-a1", "O parser lê as linhas em `src/parser.rs`, na `parse_line`. Vou rodar os testes dele.", ts=NOW - 200),
    call("z-rd", "zrd", "Read", {"file_path": "/sintetica/projetos/hangar-sintetico/src/parser.rs"}),
    result("z-rd-r", "zrd", "\n".join(f"linha {i}" for i in range(40))),
    call("z-bf", "zbf", "Bash", {"command": "cargo test -p parser-sintetico", "description": "Testar o parser"}),
    result("z-bf-r", "zbf", "error[E0425]: cannot find value `reader` in this scope (sintético)", True),
    msg("assistant_msg", "z-a2", "Um teste não compila. Peço a um subagente para revisar o módulo inteiro.", ts=NOW - 160),
    call("z-ag", "zag", "Agent", {"description": "Revisar o parser", "prompt": "Revise o módulo do parser (sintético).",
                                   "subagent_type": "general-purpose"}),
    result("z-ag-r", "zag", "Revisão concluída: 2 problemas (sintético)."),
    msg("assistant_msg", "z-a3", "A revisão achou dois problemas. Corrijo e rodo de novo.", ts=NOW - 60),
    msg("thinking", "z-th2", "Falta o `let reader` antes do laço; depois disso o teste deve compilar (sintético)."),
    call("z-rd2", "zrd2", "Read", {"file_path": "/sintetica/projetos/hangar-sintetico/src/parser.rs"}),
    result("z-rd2-r", "zrd2", "\n".join(f"linha {i}" for i in range(12))),
    call("z-bw", "zbw", "Bash", {"command": "cargo test -p parser-sintetico", "description": "Testar de novo"}),
]


def put(value):
    busy = "idle" if value == "idle" else "working"
    data = info(NAME, "claude", state=busy, branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    line = "🤖 Opus5.5·1M (high✦) │ 📁 hangar-sintetico [main] │ 💵 $0.40"
    events = [dict(e) for e in EVENTS if not (value == "between" and e["id"] == "z-bw")]
    SESSIONS[NAME] = {"info": data, "state": state(busy, status_line=line), "events": events,
                      "stats": None, "modes": [], "model": "Opus 5.5", "scene": value}
    bump()


put("working")


class Handler(T15NS["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t35":
            value = parse_qs(url.query).get("state", ["working"])[0]
            with LOCK:
                put(value if value in ("idle", "between") else "working")
            self.send_json({"state": SESSIONS[NAME]["state"]["state"], "scene": SESSIONS[NAME]["scene"]})
            return
        super().do_GET()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
