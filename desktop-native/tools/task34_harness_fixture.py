"""Harnesses SINTÉTICOS da Task 34: nenhum CLI é lido e nenhum conserto roda; tudo é texto guardado aqui.

Base: a fixture da Task 14 (sessões e servidor), carregada sem o bloco que sobe o servidor. Os nomes das contas Claude
vêm de parity_accounts_fixture, só lidos.

GET /api/harness, GET /api/harness/instalar (ocioso, só para a régua web) e POST /api/harness/conserto/<id> respondem daqui.
GET /control/t34 muda as próximas respostas (só as chaves dadas mudam):
  list=<ok|empty|500|drop>  repair=<ok|vanish|400|500|drop>  list_delay=<s>  repair_delay=<s>  reset=1
  "vanish": o conserto dá certo e o item deixa de existir (o desfecho fica no card).
  "drop" no conserto: o estado muda e a conexão cai sem resposta.
Cada pedido às rotas acima vai para visual/task34/fixture-<porta>.log.
"""
import copy
import os
import pathlib
import time
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task14_sidebar_fixture.py").read_text(encoding="utf-8")
BASE_NS = {"__name__": "task14_base", "__file__": str(HERE / "task14_sidebar_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task14_sidebar_fixture.py", "exec"), BASE_NS)
import parity_accounts_fixture as accounts  # noqa: E402  (só leitura)

BASE = BASE_NS["BASE"]
LOCK = BASE["LOCK"]
LOG_DIR = pathlib.Path.home() / ".hangar/orq/2026-09-23-native-parity/visual/task34"


def item(id_, ok, codigo, conserto=None, info=False, **params):
    return {"id": id_, "ok": ok, "codigo": codigo, "params": {k: str(v) for k, v in params.items()},
            "conserto": conserto, "info": info}


def initial():
    labels = sorted(accounts._labels())
    return [
        {"id": "claude", "nome": "Claude Code", "instalado": True, "versao": "2.1.195 (Claude Code)", "itens": [
            item("hooks", False, "faltam", "hooks-claude",
                 lista="ask-question, state, preview, subagent, pair, nav, guard-read-sem-visao-com-nome-bem-comprido"),
            item("contas", True, "contas_ok", "contas", n=len(labels), lista=", ".join(labels)),
            item("plugins", None, "config_ilegivel"),
            item("statusline", True, "statusline_ok"),
            item("fullscreen", False, "fullscreen_claude_desligado", "fullscreen:claude"),
        ]},
        {"id": "codex", "nome": "Codex", "instalado": True, "versao": "codex-cli 0.130.0", "itens": [
            item("credenciais", False, "credenciais_faltam", "sync:codex", tem="padrão", faltam="claude-200-1, 02-200"),
            item("hooks", True, "hooks_codex", info=True, n=2, eventos="SessionStart, Stop"),
            item("mcp", True, "mcp_ok", info=True, n=3, lista="hangar, context7, serena"),
            item("modelo", True, "modelo_padrao", info=True, modelo="gpt-sintetico-6"),
        ]},
        {"id": "pi", "nome": "Pi", "instalado": True, "versao": "", "itens": [
            item("skills", False, "links_pendurados", "skills", n=3, total=41),
            item("extensoes", True, "extensoes_ok", n=4),
            item("credenciais", True, "credenciais_ok", tem="padrão"),
            item("novidade", True, "codigo_que_o_app_nao_conhece"),
        ]},
        {"id": "omp", "nome": "oh-my-pi", "instalado": True, "versao": "omp 1.4.2", "itens": [
            item("extensoes", False, "extensoes_outra_fonte", "extensoes:omp", lista="hangar-state", faltam="hangar-nav"),
            item("fullscreen", False, "fullscreen_desligado", "fullscreen:omp"),
        ]},
        {"id": "kimi", "nome": "Kimi", "instalado": False, "versao": None, "itens": []},
        {"id": "tmux", "nome": "tmux", "instalado": True, "versao": "tmux 3.5a", "itens": [
            item("bloco", True, "tmux_bloco_ok"),
            item("default_terminal", False, "tmux_term_ruim", "tmux", valor="screen-256color"),
            item("mouse", True, "tmux_mouse_on", info=True),
        ]},
    ]


# O que o conserto deixa no lugar do item, por id de conserto.
FIXED = {"hooks-claude": item("hooks", True, "hooks_ok", n=7), "skills": item("skills", True, "skills_ok", n=41),
         "sync:codex": item("credenciais", True, "credenciais_ok", tem="padrão, claude-200-1, 02-200"),
         "extensoes:omp": item("extensoes", True, "extensoes_ok", n=2),
         "fullscreen:claude": item("fullscreen", True, "fullscreen_ok"), "fullscreen:omp": item("fullscreen", True, "fullscreen_ok"),
         "tmux": item("default_terminal", True, "tmux_term_ok", valor="tmux-256color")}
STATE = {"list": "ok", "repair": "ok", "list_delay": 0.0, "repair_delay": 0.0, "clis": initial()}


def log_line(port, line):
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    with (LOG_DIR / f"fixture-{port}.log").open("a", encoding="utf-8") as log:
        log.write(f"{time.strftime('%H:%M:%S')} {line}\n")


class Handler(BASE_NS["Handler"]):
    def server_error(self):
        body = b"Internal Server Error"
        self.send_response(500)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t34":
            query = parse_qs(url.query)
            with LOCK:
                if query.get("list", [""])[0] in ("ok", "empty", "500", "drop"):
                    STATE["list"] = query["list"][0]
                if query.get("repair", [""])[0] in ("ok", "vanish", "400", "500", "drop"):
                    STATE["repair"] = query["repair"][0]
                for key in ("list_delay", "repair_delay"):
                    if key in query:
                        STATE[key] = min(max(float(query[key][0]), 0.0), 15.0)
                if query.get("reset"):
                    STATE.update(list="ok", repair="ok", list_delay=0.0, repair_delay=0.0, clis=initial())
                view = {k: v for k, v in STATE.items() if k != "clis"}
            self.send_json(view)
            return
        if url.path == "/api/harness/instalar":
            # Só para a régua web montar os cards: estado ocioso de harness_install, nada a instalar por botão.
            if self.authorized():
                self.send_json({"fase": "ocioso", "harness": None, "etapa": None, "passo": 0, "total": 4, "log": [],
                                "avisos": [], "ok": None, "erro": None, "comandos": {}, "manual": {}})
            return
        if url.path != "/api/harness":
            return super().do_GET()
        log_line(self.server.server_port, f"GET {url.path}")
        if not self.authorized():
            return
        with LOCK:
            mode, delay, clis = STATE["list"], STATE["list_delay"], copy.deepcopy(STATE["clis"])
        time.sleep(delay)
        if mode == "drop":
            self.close_connection = True
        elif mode == "500":
            self.server_error()
        else:
            self.send_json([] if mode == "empty" else clis)

    def do_POST(self):
        path = urlparse(self.path).path
        if not path.startswith("/api/harness/conserto/"):
            return super().do_POST()
        repair_id = unquote(path[len("/api/harness/conserto/"):])
        log_line(self.server.server_port, f"POST conserto {repair_id}")
        if not self.authorized():
            return
        with LOCK:
            mode, delay = STATE["repair"], STATE["repair_delay"]
            # "contas" é Refazer num item já ok: responde feito e o item fica como estava.
            known = repair_id in FIXED or repair_id == "contas"
            if known and mode in ("ok", "vanish", "drop"):
                for cli in STATE["clis"]:
                    for i, it in enumerate(cli["itens"]):
                        if it["conserto"] == repair_id:
                            if mode == "vanish":
                                del cli["itens"][i]
                            elif repair_id in FIXED:
                                cli["itens"][i] = dict(FIXED[repair_id])
                            break
            clis = copy.deepcopy(STATE["clis"])
        time.sleep(delay)
        if not known or mode == "400":
            motivo = f"conserto desconhecido: {repair_id}" if not known else "o conserto sintético recusou: nada foi gravado"
            self.send_json({"detail": {"code": "erro_harness_conserto", "params": {"motivo": motivo}, "msg": motivo}}, 400)
        elif mode == "500":
            motivo = "sintético: config.toml sem permissão de escrita"
            self.send_json({"detail": {"code": "erro_harness_conserto", "params": {"motivo": motivo}, "msg": motivo}}, 500)
        elif mode == "drop":
            self.close_connection = True
        else:
            self.send_json({"feito": f"sintético: {repair_id} refeito, 3 arquivos gravados", "harnesses": clis})


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK34_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
