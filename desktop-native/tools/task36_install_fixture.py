"""Instalação SINTÉTICA de harness da Task 36: nenhum comando roda; etapas, saída e desfecho são texto guardado aqui.

Base: a fixture da Task 34 (lista de harnesses e consertos), carregada sem o bloco que sobe o servidor. Aqui o Kimi
tem comando conferido (botão) e o oh-my-pi aparece não instalado e sem comando (só o endereço do fornecedor).

GET /api/harness/instalar e POST /api/harness/instalar/<cli> copiam o formato de backend/app/harness_install.py.
GET /control/t36 muda as próximas instalações (só as chaves dadas mudam):
  reset=1 (aplicado antes das outras)  mode=<ok|fail|warn|409|400|500|drop>  step=<s por linha de saída>  lines=<n linhas>
  "fail": para em "conferir" com a mensagem longa do PATH.  "warn": termina com a etapa do wrapper pulada (aviso).
  "409"/"400"/"500": o POST é recusado com o corpo do backend.  "drop": o POST cai sem resposta e a instalação começa.
  busy=<cli>: finge uma instalação de outro CLI rodando (a de outro aparelho), até reset.
Cada pedido às rotas acima vai para visual/task36/fixture-<porta>.log.
"""
import copy
import os
import pathlib
import threading
import time
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task34_harness_fixture.py").read_text(encoding="utf-8")
T34 = {"__name__": "task34_base", "__file__": str(HERE / "task34_harness_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task34_harness_fixture.py", "exec"), T34)

BASE, LOCK, STATE = T34["BASE"], T34["LOCK"], T34["STATE"]
LOG_DIR = pathlib.Path.home() / ".hangar/orq/2026-09-23-native-parity/visual/task36"
T34["LOG_DIR"] = LOG_DIR  # as rotas herdadas da 34 também gravam aqui, nunca na pasta de prova dela
ETAPAS = ("comando", "conferir", "wrapper", "ajustes")
COMANDOS = {"kimi": "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash"}
MANUAL = {"codex": "https://github.com/openai/codex", "pi": "https://pi.dev/docs/latest",
          "omp": "https://github.com/can1357/oh-my-pi", "kimi": "https://kimi.com/code"}
LONG_PATH = ("/home/sintetico/.local/bin:/home/sintetico/.cargo/bin:/home/sintetico/.local/share/fnm/node-versions/v22.12.0/"
             "installation/bin:/usr/local/sbin:/usr/local/bin:/usr/bin:/usr/lib/jvm/default/bin:/usr/bin/site_perl")


def clis():
    out = T34["initial"]()
    for cli in out:
        if cli["id"] == "omp":
            cli.update(instalado=False, versao=None, itens=[])
    return out


def zerado(**campos):
    return {"fase": "ocioso", "harness": None, "etapa": None, "passo": 0, "total": len(ETAPAS), "log": [], "avisos": [],
            "ok": None, "erro": None, **campos}


CONF = {"mode": "ok", "step": 0.35, "lines": 24}
INST = {"estado": zerado(), "run": 0}
STATE["clis"] = clis()


def log_line(port, line):
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    with (LOG_DIR / f"fixture-{port}.log").open("a", encoding="utf-8") as log:
        log.write(f"{time.strftime('%H:%M:%S')} {line}\n")


def pub(run, **campos):
    """Nada muda depois de um reset: a rodada velha perde a vez."""
    with LOCK:
        if INST["run"] == run:
            INST["estado"] = {**INST["estado"], **campos}
            return True
    return False


def note(run, text):
    with LOCK:
        if INST["run"] != run:
            return False
        INST["estado"] = {**INST["estado"], "log": [*INST["estado"]["log"], text][-400:]}
    return True


def run_install(run, cli, mode, step, lines):
    note(run, f"$ {COMANDOS[cli]}")
    for n in range(lines):
        time.sleep(step)
        text = (f"sintético: baixando kimi-code-linux-x64 ({(n + 1) * 7} de {lines * 7} MB)" if n % 5 else
                f"sintético: https://code.kimi.com/kimi-code/releases/1.9.{n}/kimi-code-linux-x64.tar.gz?assinatura=" + "a1b2c3" * 12)
        if not note(run, text):
            return
    pub(run, etapa="conferir", passo=2)
    time.sleep(step * 3)
    if mode == "fail":
        pub(run, fase="pronto", ok=False, erro="o comando terminou, mas o CLI não aparece nem no PATH deste serviço "
                                              f"({LONG_PATH}) nem na pasta de config dele")
        return
    pub(run, etapa="wrapper", passo=3)
    time.sleep(step * 3)
    if mode == "warn":
        note(run, "[wrapper pulado] sintético: install-claude-wrapper.sh não encontrado neste checkout")
        pub(run, avisos=["a etapa do wrapper foi pulada: sintético: install-claude-wrapper.sh não encontrado neste checkout"])
    else:
        note(run, "sintético: wrappers de bash, zsh e fish atualizados")
    pub(run, etapa="ajustes", passo=4)
    time.sleep(step * 3)
    note(run, "$ skills\nsintético: 41 skills ligadas ao Kimi")
    with LOCK:
        if INST["run"] != run:
            return
        for c in STATE["clis"]:
            if c["id"] == cli:
                c.update(instalado=True, versao="kimi-code 1.9.24", itens=[
                    T34["item"]("skills", True, "skills_ok", n=41),
                    T34["item"]("hooks", True, "hooks_ok", n=4)])
        INST["estado"] = {**INST["estado"], "fase": "pronto", "ok": True, "erro": None}


class Handler(T34["Handler"]):
    def status(self):
        with LOCK:
            return {**copy.deepcopy(INST["estado"]), "comandos": dict(COMANDOS), "manual": dict(MANUAL)}

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t36":
            query = parse_qs(url.query)
            with LOCK:
                if query.get("reset"):
                    CONF.update(mode="ok", step=0.35, lines=24)
                    INST["run"] += 1
                    INST["estado"] = zerado()
                    STATE.update(list="ok", repair="ok", list_delay=0.0, repair_delay=0.0, clis=clis())
                if query.get("mode", [""])[0] in ("ok", "fail", "warn", "409", "400", "500", "drop"):
                    CONF["mode"] = query["mode"][0]
                if "step" in query:
                    CONF["step"] = min(max(float(query["step"][0]), 0.0), 5.0)
                if "lines" in query:
                    CONF["lines"] = min(max(int(query["lines"][0]), 0), 400)
                if query.get("busy"):
                    INST["run"] += 1
                    INST["estado"] = zerado(fase="rodando", harness=query["busy"][0], etapa="comando", passo=1)
                view = {**CONF, "fase": INST["estado"]["fase"], "harness": INST["estado"]["harness"]}
            self.send_json(view)
            return
        if url.path != "/api/harness/instalar":
            return super().do_GET()
        log_line(self.server.server_port, f"GET {url.path}")
        if self.authorized():
            self.send_json(self.status())

    def do_POST(self):
        path = urlparse(self.path).path
        if not path.startswith("/api/harness/instalar/"):
            return super().do_POST()
        cli = unquote(path[len("/api/harness/instalar/"):])
        log_line(self.server.server_port, f"POST instalar {cli}")
        if not self.authorized():
            return
        with LOCK:
            mode, step, lines = CONF["mode"], CONF["step"], CONF["lines"]
            estado = INST["estado"]
            busy = estado["fase"] == "rodando" and estado["harness"] != cli
            again = estado["fase"] == "rodando" and estado["harness"] == cli
        if mode == "400" or cli not in COMANDOS:
            self.send_json({"detail": {"code": "erro_harness_sem_instalador", "params": {"cli": cli},
                                       "msg": f"nao ha comando conferido pra instalar {cli} aqui"}}, 400)
            return
        if mode == "409" or busy:
            outro = estado["harness"] if busy else "omp"
            self.send_json({"detail": {"code": "erro_harness_instalando", "params": {"harness": outro},
                                       "msg": f"ja ha uma instalacao em curso ({outro}) — espere ela terminar"}}, 409)
            return
        if mode == "500":
            self.server_error()
            return
        if not again:
            with LOCK:
                INST["run"] += 1
                run = INST["run"]
                INST["estado"] = zerado(fase="rodando", harness=cli, etapa=ETAPAS[0], passo=1)
            threading.Thread(target=run_install, args=(run, cli, mode, step, lines), daemon=True).start()
        if mode == "drop":
            self.close_connection = True
            return
        self.send_json(self.status(), 202)


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK36_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
