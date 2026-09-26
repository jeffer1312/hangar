"""Integração do Codex SINTÉTICA da Task 40: nenhuma rodada roda de verdade; o estado é texto guardado aqui.

Base: a fixture da Task 38 (opções do Claude, harnesses da Task 34), carregada sem o bloco que sobe o servidor.

GET e POST /api/harness/codex/integracao respondem daqui, com `automatica`/`memoria` lidos do /api/config daqui.
POST /api/config com `codex_sync` ou `codex_memory_import` grava aqui; as chaves do Claude seguem para a Task 38.
Reconciliar começa uma rodada que anda uma etapa a cada leitura (5 etapas, sub-andamento na 4ª) e termina em `outcome`.
GET /control/t40 muda as próximas respostas (só as chaves dadas mudam):
  integ=<ok|idle|unavailable|500|drop>  reconcile=<ok|500|drop>  save=<ok|400|500|drop>  outcome=<ok|parcial|erro>
  trust=<0|1>  integ_delay=<s>  reconcile_delay=<s>  save_delay=<s>  reset=1
  "idle": nunca executada. "unavailable": sem Codex nesta máquina. "drop" na gravação: o valor muda e a conexão cai.
  Erros com o corpo do backend: 400 `{"detail": "<campo>: esperado true/false"}` (ValueError do runtime_config) e
  500 em texto puro `Internal Server Error` (o Starlette, para qualquer outra exceção). As rotas da integração não têm
  erro próprio no backend: só o 500 do Starlette e a queda.
Cada pedido às rotas acima vai para visual/task40/fixture-<porta>.log.
"""
import io
import json
import os
import pathlib
import time
from datetime import datetime, timedelta, timezone
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task38_claude_options_fixture.py").read_text(encoding="utf-8")
T38 = {"__name__": "task38_base", "__file__": str(HERE / "task38_claude_options_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task38_claude_options_fixture.py", "exec"), T38)

BASE, LOCK = T38["BASE"], T38["LOCK"]
T38["T34"]["LOG_DIR"] = pathlib.Path.home() / ".hangar/orq/2026-09-23-native-parity/visual/task40"
log_line = T38["log_line"]
KEYS = ("codex_sync", "codex_memory_import")
PATH = "/api/harness/codex/integracao"


def message(code, **params):
    return {"codigo": code, "params": params, "texto": code}


# Uma leitura por passo: (etapa, código da etapa, parâmetros, sub-andamento).
RUN = [
    (1, "etapa_inventariando", {}, None),
    (2, "etapa_instrucoes", {}, None),
    (3, "etapa_fragmentos", {}, None),
    (4, "etapa_plugin", {"id": "superpowers@claude-plugins"}, (1, 3)),
    (4, "etapa_plugin", {"id": "context7@claude-plugins"}, (2, 3)),
    (4, "etapa_plugin", {"id": "example@sample-marketplace"}, (3, 3)),
    (5, "etapa_skills", {}, None),
]

PLUGINS = [
    {"id": "superpowers@claude-plugins", "versao": "5.0.7", "origem": "claude"},
    {"id": "context7@claude-plugins", "versao": "1.2.0", "origem": "claude"},
    {"id": "example@sample-marketplace", "versao": "2026.09.24-com-um-nome-de-versao-bem-longo-para-quebrar", "origem": "marketplace"},
]


def iso(delta=timedelta()):
    return (datetime.now(timezone.utc) + delta).isoformat()


def initial():
    return {"integ": "ok", "reconcile": "ok", "save": "ok", "outcome": "ok", "trust": False,
            "integ_delay": 0.0, "reconcile_delay": 0.0, "save_delay": 0.0,
            "values": {"codex_sync": True, "codex_memory_import": False},
            "run": None, "last": iso(timedelta(hours=-3)), "last_outcome": "ok"}


STATE = initial()


def snapshot():
    """O que `SERVICO.status()` + `_com_interruptor` devolvem agora; uma leitura durante a rodada a faz andar um passo."""
    base = {"estado": "ok", "etapa": message("etapa_concluido"), "ultima_execucao": STATE["last"],
            "proxima_atualizacao": iso(timedelta(minutes=27)), "plugins": PLUGINS, "erros": [],
            "avisos": [message("aviso_hooks_sem_arquivo", arquivos="pre-commit.sh, notify-send.py")],
            "confianca_pendente": STATE["trust"], "progresso": None, "etapa_segundos": None,
            "skills": {"ponte": 14, "nativas": 3},
            "automatica": STATE["values"]["codex_sync"], "memoria": STATE["values"]["codex_memory_import"]}
    if STATE["integ"] == "idle":
        base.update(estado="ocioso", etapa=message("etapa_aguardando"), ultima_execucao=None, proxima_atualizacao=None,
                    plugins=[], avisos=[], skills={"ponte": 0, "nativas": 0})
    elif STATE["integ"] == "unavailable":
        base.update(estado="indisponivel", etapa=message("etapa_sem_codex"), proxima_atualizacao=None)
    run = STATE["run"]
    if run is not None:
        if run["step"] < len(RUN):
            passo, code, params, sub = RUN[run["step"]]
            run["step"] += 1
            base.update(estado="executando", etapa=message(code, **params), etapa_segundos=int(time.time() - run["since"]),
                        progresso={"passo": passo, "total": 5, "sub": {"atual": sub[0], "total": sub[1]} if sub else None})
            return base
        STATE["run"], STATE["last"], STATE["last_outcome"] = None, iso(), STATE["outcome"]
        base["ultima_execucao"] = STATE["last"]
    if STATE["integ"] == "ok" and STATE["last_outcome"] != "ok":
        base["estado"] = STATE["last_outcome"]
        base["etapa"] = message("etapa_pendencias")
        base["erros"] = [message("erro_plugin", id="example@sample-marketplace")] if STATE["last_outcome"] == "erro" \
            else [message("erro_marketplace_pendente", marketplace="sample-marketplace")]
    return base


class Handler(T38["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t40":
            query = parse_qs(url.query)
            choices = {"integ": ("ok", "idle", "unavailable", "500", "drop"), "reconcile": ("ok", "500", "drop"),
                       "save": ("ok", "400", "500", "drop"), "outcome": ("ok", "parcial", "erro")}
            with LOCK:
                for key, allowed in choices.items():
                    if query.get(key, [""])[0] in allowed:
                        STATE[key] = query[key][0]
                if "trust" in query:
                    STATE["trust"] = query["trust"][0] == "1"
                for key in ("integ_delay", "reconcile_delay", "save_delay"):
                    if key in query:
                        STATE[key] = min(max(float(query[key][0]), 0.0), 15.0)
                if query.get("reset"):
                    STATE.clear()
                    STATE.update(initial())
                view = {k: v for k, v in STATE.items() if k != "run"}
            self.send_json(view)
            return
        if url.path != PATH:
            return super().do_GET()
        log_line(self.server.server_port, f"GET {PATH}")
        if not self.authorized():
            return
        with LOCK:
            mode, delay = STATE["integ"], STATE["integ_delay"]
            body = snapshot() if mode not in ("500", "drop") else None
        self.reply("ok" if body is not None else mode, delay, body)

    def do_POST(self):
        path = urlparse(self.path).path
        if path == PATH:
            log_line(self.server.server_port, f"POST {PATH}")
            if not self.authorized():
                return
            with LOCK:
                mode, delay = STATE["reconcile"], STATE["reconcile_delay"]
                if mode in ("ok", "drop") and STATE["run"] is None:
                    STATE["run"] = {"step": 0, "since": time.time()}
                body = snapshot() if mode == "ok" else None
            self.reply(mode, delay, body)
            return
        if path != "/api/config":
            return super().do_POST()
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) or b"{}"
        body = json.loads(raw)
        if not any(k in KEYS for k in body):
            # Pedido do Claude: devolve o corpo lido para a fixture da Task 38 responder.
            self.rfile = io.BytesIO(raw)
            return super().do_POST()
        log_line(self.server.server_port, f"POST /api/config {json.dumps(body, sort_keys=True)}")
        if not self.authorized():
            return
        with LOCK:
            mode, delay = STATE["save"], STATE["save_delay"]
            if mode in ("ok", "drop"):
                STATE["values"].update({k: bool(v) for k, v in body.items() if k in KEYS})
            fields = {k: {"valor": v, "origem": "app"} for k, v in STATE["values"].items()}
        self.reply(mode, delay, {"campos": fields, "somente_leitura": {}}, next(iter(body), None))


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK40_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
