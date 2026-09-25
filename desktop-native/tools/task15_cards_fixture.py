"""Fixture SINTÉTICA da Task 15d (cartões de mensagem): a da Task 22 com uma sessão cuja conversa traz as mensagens de
usuário que viram cartão. Nenhuma sessão, pasta, agente ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 22 sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.

Sessão sintetica-cartoes, notificações de subagente do Codex (`<subagent_notification>{json}</subagent_notification>`):
  concluída com relatório em markdown (título, negrito, lista, código), falha (`errored`, sem agent_path), falha
  (`failed`), status desconhecido com corpo objeto (vira JSON legível), malformada (JSON quebrado: bolha), com texto antes
  do envelope (bolha), uma com cru acima de 20 000 caracteres (bloco aberto cortado, com a nota) e uma na fila
  (`queued-`: bolha, como no web).
"""
import json
import pathlib
import time

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


def add_rows():
    data = info(NAME, "codex", state="idle", branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    SESSIONS[NAME] = {"info": data, "state": state("idle"), "events": [dict(e) for e in EVENTS], "stats": None, "modes": []}
    bump()


BASE_FILL = T22NS["fill"]


def fill():
    BASE_FILL()
    add_rows()


T22NS["fill"] = fill
add_rows()

server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), T22NS["Handler"])
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
