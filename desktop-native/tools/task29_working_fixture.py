"""Fixture SINTÉTICA da Task 29 (linha "trabalhando" com verbo e segundos, "Enviando…", pílula "Ir para o fim"): a da
Task 15a com o envio virando turno. Nenhuma sessão, pasta, agente ou conta daqui existe de verdade; nada sai deste
processo.

Carrega o código da fixture da Task 15a sem o bloco que sobe o servidor (o arquivo dela não muda) e estende o Handler.
Tudo o que ela oferece continua valendo na sessão sintetica-agentes (GET /control/t15?do=…: working, idle, label,
thinking, tool, preview, long, calm…).

A mais, na sintetica-agentes:
- a primeira mensagem tem hora (20 s atrás): a conversa aberta já trabalhando conta dali;
- o envio responde em 1,5 s (herdado) e o turno começa 2 s depois da resposta, com o rótulo "Computing… (Ns)": dá
  para ver o "Enviando…" durante a entrega e na ponte até o turno.
"""
import pathlib
import time
from urllib.parse import unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task15_working_fixture.py").read_text(encoding="utf-8")
T15NS = {"__name__": "task15_working_base", "__file__": str(HERE / "task15_working_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task15_working_fixture.py", "exec"), T15NS)

BASE = T15NS["BASE"]
NAME = T15NS["NAME"]
T15NS["START"][0]["ts"] = time.time() - 20
T15NS["scene_reset"]()


class Handler(T15NS["Handler"]):
    def do_POST(self):
        super().do_POST()
        parts = [unquote(p) for p in urlparse(self.path).path.strip("/").split("/")]
        if parts[-1:] == ["input"] and NAME in parts:
            BASE["later"](2.0, lambda: T15NS["apply"]("label"))


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
