"""Fixture SINTÉTICA da Task 31 (hora e copiar sob a mensagem, bolha que recolhe, anéis do rodapé): a da Task 15d com
quatro sessões a mais. Nenhuma sessão, pasta, agente ou conta daqui existe de verdade; nada sai deste processo.

Carrega o código da fixture da Task 15d sem o bloco que sobe o servidor (o arquivo dela não muda).

Sessões sintetica-anel-0, -50, -95 e -sem-dado: a mesma conversa (mensagem longa do usuário, que recolhe; resposta do
agente; mensagem curta de hoje e uma de três dias atrás), com contexto e uso da conta (5h) em 0%, 50%, 95% e sem os
dois campos na linha de status.
"""
import pathlib
import time

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task15_cards_fixture.py").read_text(encoding="utf-8")
T15NS = {"__name__": "task15_base", "__file__": str(HERE / "task15_cards_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task15_cards_fixture.py", "exec"), T15NS)

BASE = T15NS["BASE"]
SESSIONS, bump, info, state, msg = (BASE[k] for k in ("SESSIONS", "bump", "info", "state", "msg"))

NOW = time.time()

LOG = "\n".join(f"linha {n} do log colado (sintético): etapa {n} concluída sem erro" for n in range(1, 13))

EVENTS = [
    msg("user_msg", "m0", "Mensagem de três dias atrás (sintética).", ts=NOW - 3 * 86_400),
    msg("assistant_msg", "m1", "Resposta antiga do agente (sintética).", ts=NOW - 3 * 86_400 + 30),
    msg("user_msg", "m2", "Olha esse log e me diz se algo falhou:\n\n" + LOG, ts=NOW - 300),
    msg("assistant_msg", "m3", "Nenhuma etapa falhou: as 12 linhas terminam em **concluída sem erro**.", ts=NOW - 280),
    msg("user_msg", "m4", "Obrigado.", ts=NOW - 60),
]

RINGS = {
    "sintetica-anel-0": " │ 💬 0k/0 0k/1M │ ⚡5h:0% ↺4h 📅7d:0% ↺sab 18h",
    "sintetica-anel-50": " │ 💬 8k/600 500k/1M │ ⚡5h:50% ↺2h 📅7d:50% ↺sab 18h",
    "sintetica-anel-95": " │ 💬 8k/600 950k/1M │ ⚡5h:95% ↺20m 📅7d:95% ↺sab 18h",
    "sintetica-anel-sem-dado": "",
}

for name, rings in RINGS.items():
    data = info(name, "claude", state="idle", branch="main")
    data["cwd"] = "/sintetica/projetos/hangar-sintetico"
    line = f"🤖 Opus5.5·1M (high✦) │ 📁 hangar-sintetico [main]{rings} │ 💵 $1.20"
    SESSIONS[name] = {"info": data, "state": state("idle", status_line=line), "events": [dict(e) for e in EVENTS],
                      "stats": None, "modes": [], "model": "Opus 5.5"}
bump()

server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), T15NS["Handler"])
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
