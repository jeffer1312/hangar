"""Fixture SINTÉTICA da Task 33 (subagentes: rodando, concluído, falhou, órfão; cartão Agent com a marca e ↗): a da
Task 15b/c/d (task15_activity_fixture.py, que não muda) com o campo `failed` do Claude em /subagents. Nenhuma sessão,
pasta, agente ou conta daqui existe de verdade; nada sai deste processo.

Tudo o que a 15 oferece continua valendo na sessão sintetica-agentes (GET /control/t15?do=…, /control/t15b, /control/t15c).

A mais:
GET /control/t33?field=1|0   com ou sem o campo `failed` na lista e no detalhe (0 = servidor antigo, sem o campo)
GET /control/t15?do=fail     Agent em primeiro plano cujo subagente (ag-fail1) acaba em erro de API
GET /control/t15?do=fail_end o resultado dele no pai, sem is_error (como o Claude Code manda)
Subagentes novos na lista: ag-fail1 (casa com o `fail`, 3 comandos — o grupo pede 3 —, 1 com erro, fim em erro de API) e ag-orf-fail
(órfão, sem Agent no pai, falhou). O ag-orf-fail tem o título mais longo da fixture.
"""
import pathlib
from urllib.parse import parse_qs, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task15_activity_fixture.py").read_text(encoding="utf-8")
T15B = {"__name__": "task15_activity_base", "__file__": str(HERE / "task15_activity_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task15_activity_fixture.py", "exec"), T15B)

BASE, LOCK, T15 = T15B["BASE"], T15B["LOCK"], T15B["T15"]
sub, ev, use, out = (T15B[k] for k in ("sub", "ev", "use", "out"))
FIELD = {"on": True}
FAIL_PROMPT = "Rode a suíte sintética e conte as falhas."
LONG_TITLE = ("Revisar a migração sintética inteira do módulo de faturamento, tabela por tabela, conferindo cada índice "
              "e cada gatilho contra o esquema antigo")

BASE_LISTING = T15B["listing"]
BASE_DETAIL = T15B["detail"]


def with_field(item):
    """O dicionário do Claude traz `failed` sempre (menos o ilegível); o servidor antigo não traz."""
    item = {k: v for k, v in item.items() if k != "failed"}
    if FIELD["on"] and not item.get("ilegivel"):
        item["failed"] = item.get("_failed", False)
    item.pop("_failed", None)
    return item


def raw_listing():
    return BASE_LISTING() + [
        sub("ag-fail1", FAIL_PROMPT, 3, ["Bash", "Bash", "Bash"], agent_type="general-purpose", _failed=True,
            tools=[{"name": "Bash", "count": 3}]),
        sub("ag-orf-fail", LONG_TITLE, 5, ["Read", "Bash"], _failed=True),
    ]


def listing():
    return [with_field(item) for item in raw_listing()]


def detail(agent_id):
    if agent_id not in ("ag-fail1", "ag-orf-fail"):
        body = BASE_DETAIL(agent_id)
        return with_field(body) if body else body
    base = next(s for s in raw_listing() if s["agentId"] == agent_id)
    events = [
        ev("user_msg", "u-0301", base["prompt"]),
        use("a-0301", "w1", "Bash", {"command": "cargo test -p sintetico 2>&1 | tail -3"}),
        use("a-0301:1", "w2", "Bash", {"command": "cargo test -p sintetico-web 2>&1 | tail -3"}),
        use("a-0301:2", "w3", "Bash", {"command": "cargo test -p sintetico-cli 2>&1 | tail -3"}),
        out("r-0301", "w1", "test result: ok. 12 passed (sintético)"),
        out("r-0301:1", "w2", "error: could not compile `sintetico-web` (sintético)", error=True),
        out("r-0301:2", "w3", "test result: ok. 4 passed (sintético)"),
        ev("assistant_msg", "a-0302", "API Error: 529 Overloaded. This is a server-side issue (sintético).", is_error=True),
    ]
    return with_field({**base, "events": events})


# O detalhe e a lista da 15 leem estes nomes no próprio módulo.
T15B["listing"], T15B["detail"] = listing, detail

STEPS = T15["STEPS"]
STEPS["fail"] = [T15["agent"]("live-fail", "t-fail", "Rodar a suíte sintética", FAIL_PROMPT)]
STEPS["fail_end"] = [T15["result"]("live-fail-end", "t-fail", "API Error: 529 Overloaded (sintético)")]


class Handler(T15B["Handler"]):
    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/control/t33":
            query = parse_qs(url.query)
            with LOCK:
                FIELD["on"] = query.get("field", ["1" if FIELD["on"] else "0"])[0] == "1"
            self.send_json(dict(FIELD))
            return
        super().do_GET()


server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
