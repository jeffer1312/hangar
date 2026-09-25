"""Fixture SINTÉTICA da Task 15d (cartões de mensagem): a da Task 22 com uma sessão cuja conversa traz as mensagens de
usuário que viram cartão. Nenhuma sessão, pasta, agente ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 22 sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.

Sessão sintetica-cartoes, notificações de subagente do Codex (`<subagent_notification>{json}</subagent_notification>`):
  concluída com relatório em markdown (título, negrito, lista, código), falha (`errored`, sem agent_path), falha
  (`failed`), status desconhecido com corpo objeto (vira JSON legível), malformada (JSON quebrado: bolha), com texto antes
  do envelope (bolha), uma com cru acima de 20 000 caracteres (bloco aberto cortado, com a nota) e uma na fila
  (`queued-`: bolha, como no web).

Sessão sintetica-bastao, kick-offs da passagem de bastão (texto do `bastao.kickoff` do backend): um na fila entregue com
origem na lista (sintetica-stream), conta e modelo; um gravado com a redação antiga ("dossiê"), origem fora da lista e
sem conta/modelo; um sem uma das marcas (bolha); um na fila com `desistiu` (cartão + "não chegou" + Descartar).

GET /api/sessions/<n>/bastao/dossie responde conforme GET /control/t15d?dossie=<ok|vazio|404|500|drop>&delay=<s>
(só as chaves dadas mudam; responde o modo atual). drop fecha a conexão sem resposta.
"""
import json
import pathlib
import threading
import time
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task22_perf_fixture.py").read_text(encoding="utf-8")
T22NS = {"__name__": "task22_base", "__file__": str(HERE / "task22_perf_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task22_perf_fixture.py", "exec"), T22NS)

BASE = T22NS["BASE"]
SESSIONS, bump, info, state, msg = (BASE[k] for k in ("SESSIONS", "bump", "info", "state", "msg"))

NAME = "sintetica-cartoes"
NOW = time.time()


def codex(status, body, path="019a7f3e-5b2c-7d41-9e0a-sintetico01"):
    data = {"status": {status: body}}
    if path:
        data["agent_path"] = path
    return f"<subagent_notification>\n{json.dumps(data, ensure_ascii=False)}\n</subagent_notification>"


REPORT = ("## Resumo\n\nConferi o **parser** do envelope nos três lugares que o leem.\n\n"
          "- `subagenteCodex.ts`: casa o envelope inteiro.\n- O status com corpo objeto vira JSON legível.\n\n"
          "```rust\nlet card = cards::codex_subagent(text)?;\n```\n\nNenhuma pendência (sintético).")

EVENTS = [
    msg("user_msg", "k0", "Dispara os subagentes do Codex e me mostra o que voltou.", ts=NOW - 900),
    msg("assistant_msg", "k1", "Disparei três subagentes; as notificações chegam abaixo.", ts=NOW - 880),
    msg("user_msg", "k2", codex("completed", REPORT), ts=NOW - 840),
    msg("user_msg", "k3", codex("errored", "O subagente parou: **timeout** ao ler `/sintetica/a.rs`.", path=None), ts=NOW - 780),
    msg("user_msg", "k4", codex("failed", "Falhou ao abrir o arquivo (sintético)."), ts=NOW - 720),
    msg("user_msg", "k5", codex("running", {"passo": 2, "de": 5}), ts=NOW - 660),
    msg("user_msg", "k6", "<subagent_notification>\n{\"status\": {\"completed\": \"quebrado\"\n</subagent_notification>", ts=NOW - 600),
    msg("user_msg", "k7", "antes " + codex("completed", "texto antes do envelope"), ts=NOW - 540),
    # Cru acima do teto de 20 000 caracteres: o bloco aberto corta e diz quanto mostra.
    msg("user_msg", "k7b", codex("completed", "Relatório longo (sintético). " + "linha de relatório comprido " * 900), ts=NOW - 520),
    msg("assistant_msg", "k8","Das seis notificações, duas vieram malformadas e ficaram como mensagem comum.", ts=NOW - 500),
    msg("user_msg", "queued-k9", codex("completed", "Na fila: segue bolha."), ts=NOW - 60),
]


BATON = "sintetica-bastao"
DOSSIER = "/sintetica/bastao/sintetica-stream-0925.md"


def kickoff(origin, dossier_line, from_line, marks=True):
    return "\n".join([
        f"[hangar: passagem de bastão] Você continua o trabalho da sessão `{origin}` — não é tarefa nova, é a mesma, no ponto em que ela parou.",
        dossier_line,
        ("Leia o plano e o contrato citados no resumo ANTES de mexer em qualquer arquivo — o resumo diz onde parou, o plano diz o que vem em "
         "seguida." if marks else "Leia o resumo antes de mexer em qualquer arquivo."),
        f"A sessão `{origin}` continua VIVA, mas parou de escrever: daqui pra frente quem escreve no diretório é você (um escritor por árvore).",
        "Se o resumo mostrar par ou grupo, a continuação NÃO move esses vínculos: troque a linha da tabela de papéis para o SEU nome e avise o par.",
        from_line,
    ])


NEW_LINE = f"Comece lendo, com um `Read`, o resumo do trabalho em `{DOSSIER}`: onde ele está, o que já está no disco e por que as decisões foram tomadas."
FROM_LINE = "Ela vinha de conta `sintetica-02` · modelo `claude-opus-5-5/high` — você pode estar em outra."
NO_FROM = "A conta e o modelo de onde ela vinha estão na primeira seção do resumo — você pode estar em outros."

BATON_EVENTS = [
    msg("user_msg", "queued-b1", kickoff("sintetica-stream", NEW_LINE, FROM_LINE), ts=NOW - 900, queued_delivered=True),
    msg("assistant_msg", "b2", "Li o resumo e o plano; sigo da Task 3.", ts=NOW - 860),
    msg("user_msg", "b3", kickoff("sintetica-sumiu", "Comece lendo o dossiê em `/sintetica/bastao/antigo.md`.", NO_FROM), ts=NOW - 700),
    msg("user_msg", "b4", kickoff("sintetica-stream", NEW_LINE, FROM_LINE, marks=False), ts=NOW - 500),
    msg("user_msg", "queued-b5", kickoff("sintetica-stream", NEW_LINE, FROM_LINE), ts=NOW - 60, desistiu=True),
]

DOSSIER_BODY = ("# Resumo do trabalho (sintético)\n\n## Onde está\n\n- Branch `sintetica-main`, Task 3 de 5.\n"
                "- Últimos commits já no disco.\n\n## Decisões\n\n1. Cartão com fundo de destaque, **sem faixa lateral**.\n"
                "2. Resumo lido uma vez por tela.\n\n## Próximo passo\n\nConferir a prova e seguir o plano.\n")
T15D = {"dossie": "ok", "delay": 0.0}
T15D_LOCK = threading.Lock()


def add_rows():
    data = info(NAME, "codex", state="idle", branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    SESSIONS[NAME] = {"info": data, "state": state("idle"), "events": [dict(e) for e in EVENTS], "stats": None, "modes": []}
    data = info(BATON, "claude", state="idle", branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    SESSIONS[BATON] = {"info": data, "state": state("idle"), "events": [dict(e) for e in BATON_EVENTS], "stats": None, "modes": []}
    bump()


BASE_FILL = T22NS["fill"]


def fill():
    BASE_FILL()
    add_rows()


T22NS["fill"] = fill
add_rows()

class Handler(T22NS["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t15d":
            with T15D_LOCK:
                for key, values in parse_qs(url.query).items():
                    if key in T15D:
                        T15D[key] = float(values[0]) if key == "delay" else values[0]
                current = dict(T15D)
            self.send_json(current)
            return
        parts = url.path.split("/")
        if len(parts) == 6 and parts[:3] == ["", "api", "sessions"] and parts[4:] == ["bastao", "dossie"]:
            if not self.authorized():
                return
            with T15D_LOCK:
                mode, delay = T15D["dossie"], T15D["delay"]
            time.sleep(delay)
            if mode == "drop":
                self.close_connection = True
                self.connection.shutdown(2)
                return
            if mode == "404":
                # O envelope do `erro()` do backend (mensagens.py), como a rota real responde.
                self.send_json({"detail": {"code": "erro_bastao_sem_dossie", "params": {},
                                           "msg": "no handover dossier for this session"}}, 404)
                return
            if mode == "500":
                # A rota não tem 500 próprio: exceção solta sai no texto puro do Starlette.
                data = b"Internal Server Error"
                self.send_response(500)
                self.send_header("Content-Type", "text/plain; charset=utf-8")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
            data = (DOSSIER_BODY if mode == "ok" else "").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/markdown; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        super().do_GET()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
