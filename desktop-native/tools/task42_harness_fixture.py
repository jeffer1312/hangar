"""Contas e opções Codex sintéticas da Task 42, sem ler ou gravar contas reais.

Herda a fixture da Task 40. GET /control/t42 aceita accounts=<ok|empty|500|drop>,
prepare=<ok|partial|error|409|500|drop>, options=<ok|500|drop>,
save=<ok|409|drop>, *_delay=<segundos> e reset=1. O POST de preparo registra
forcar=true/false. Pedidos novos são registrados em visual/task42/fixture-<porta>.log.
"""
import copy
import json
import os
import pathlib
import time
from urllib.parse import parse_qs, unquote, urlparse

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task40_codex_integration_fixture.py").read_text(encoding="utf-8")
T40 = {"__name__": "task40_base", "__file__": str(HERE / "task40_codex_integration_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task40_codex_integration_fixture.py", "exec"), T40)

BASE, LOCK = T40["BASE"], T40["LOCK"]
T40["T38"]["T34"]["LOG_DIR"] = pathlib.Path.home() / ".hangar/orq/2026-09-23-native-parity/visual/task42"
log_line = T40["log_line"]


def initial():
    return {"accounts": "ok", "prepare": "ok", "options": "ok", "save": "ok",
            "accounts_delay": 0.0, "prepare_delay": 0.0, "options_delay": 0.0, "save_delay": 0.0,
            "preparations": {}, "last_forcar": None,
            "contexto_estendido": False, "codex_voice_beta": False}


STATE = initial()
READY = {"status": "ready", "trust_pending": False, "issues": []}
HERDADO = {"skills": 3, "hooks": 2, "agents": 1, "plugins": 2, "mcps": 1}
ACCOUNTS = (
    ("default", True, "padrao"),
    ("pessoal", False, "pessoal"),
)


def account_path(path):
    parts = [unquote(part) for part in path.strip("/").split("/")]
    return parts[2] if len(parts) == 4 and parts[:2] == ["api", "codex-contas"] and parts[3] == "prepare" else None


def preparation(account, advance=False):
    if account == "default":
        return dict(READY)
    reads = STATE["preparations"].get(account)
    if reads is None:
        return {"status": "idle", "trust_pending": False, "issues": []}
    if reads < 3:
        if advance:
            STATE["preparations"][account] += 1
        return {"status": "running", "trust_pending": False, "issues": [],
                "etapa": ("principal", "herdar_configuracao", "recursos")[reads]}
    mode = STATE["prepare"]
    if mode == "partial":
        return {"status": "partial", "trust_pending": False, "herdado": HERDADO,
                "issues": [{"code": "codex_account_source_sync_incomplete", "params": {"status": "parcial"}}]}
    if mode == "error":
        return {"status": "error", "trust_pending": False,
                "issues": [{"code": "codex_account_prepare_failed", "params": {"error": "OSError"}}]}
    return {**READY, "herdado": HERDADO}


def accounts():
    if STATE["accounts"] == "empty":
        return []
    return [{"id": id_, "credential_id": f"codex:/prova/.codex-{slug}", "name": id_,
             "home": f"/prova/.codex-{slug}", "is_default": default,
             "auth": {"method": "oauth", "status": "connected", "email": f"{slug}@exemplo.test", "plan": "plus"},
             "sync": preparation(id_), "has_settings": True}
            for id_, default, slug in ACCOUNTS]


def options():
    extended = STATE["contexto_estendido"]
    return {"contexto_estendido": extended, "codex_voice_beta": STATE["codex_voice_beta"],
            "contexto_configurado": 1_000_000 if extended else None,
            "compactacao": 900_000 if extended else None,
            "modelos": [{"model": "gpt-sintetico-6", "default": 272_000, "max": 1_000_000}]}


class Handler(T40["Handler"]):
    def account_error(self, account):
        self.send_json({"detail": {"code": "codex_account_not_found", "params": {"account_id": account},
                                   "msg": "conta Codex não encontrada"}}, 404)

    def do_GET(self):
        url = urlparse(self.path)
        path = url.path
        if path == "/control/t42":
            query = parse_qs(url.query)
            with LOCK:
                for key, allowed in {"accounts": ("ok", "empty", "500", "drop"),
                                     "prepare": ("ok", "partial", "error", "409", "500", "drop"),
                                     "options": ("ok", "500", "drop"),
                                     "save": ("ok", "409", "drop")}.items():
                    if query.get(key, [""])[0] in allowed:
                        STATE[key] = query[key][0]
                for key in ("accounts_delay", "prepare_delay", "options_delay", "save_delay"):
                    if key in query:
                        STATE[key] = min(max(float(query[key][0]), 0.0), 15.0)
                if query.get("reset"):
                    STATE.clear()
                    STATE.update(initial())
                view = copy.deepcopy(STATE)
            self.send_json(view)
            return
        account = account_path(path)
        if path not in ("/api/codex-contas", "/api/harness/codex/opcoes") and account is None:
            return super().do_GET()
        log_line(self.server.server_port, f"GET {path}")
        if not self.authorized():
            return
        with LOCK:
            if account is not None and account not in ("default", "pessoal"):
                return self.account_error(account)
            mode = STATE["accounts"] if path == "/api/codex-contas" else STATE["options"] if account is None else STATE["prepare"]
            delay = STATE["accounts_delay"] if path == "/api/codex-contas" else STATE["options_delay"] if account is None else STATE["prepare_delay"]
            body = accounts() if path == "/api/codex-contas" else options() if account is None else preparation(account, advance=True)
        self.reply(mode if mode in ("500", "drop") else "ok", delay, body)

    def do_POST(self):
        url = urlparse(self.path)
        path = url.path
        account = account_path(path)
        if path != "/api/harness/codex/opcoes" and account is None:
            return super().do_POST()
        query = parse_qs(url.query)
        forced = query.get("forcar", ["false"])[0] == "true"
        log_line(self.server.server_port, f"POST {path}" + (f" forcar={str(forced).lower()}" if account else ""))
        if not self.authorized():
            return
        if account is not None and account not in ("default", "pessoal"):
            return self.account_error(account)
        if account is not None:
            with LOCK:
                STATE["last_forcar"] = forced
                mode, delay = STATE["prepare"], STATE["prepare_delay"]
                if mode in ("ok", "partial", "error", "drop") and account != "default":
                    STATE["preparations"][account] = 0
                body = preparation(account)
            time.sleep(delay)
            if mode == "409":
                self.send_json({"detail": {"code": "codex_account_preparing", "params": {"account_id": account},
                                           "msg": "a preparação da conta Codex está em andamento"}}, 409)
            elif mode in ("500", "drop"):
                self.reply(mode, 0, body)
            else:
                self.send_json(body, 202)
            return
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"{}")
        log_line(self.server.server_port, f"POST {path} body={json.dumps(body, sort_keys=True)}")
        with LOCK:
            mode, delay = STATE["save"], STATE["save_delay"]
            if mode in ("ok", "drop"):
                STATE["contexto_estendido"] = body["contexto_estendido"]
                if body.get("codex_voice_beta") is not None:
                    STATE["codex_voice_beta"] = body["codex_voice_beta"]
            result = options()
        time.sleep(delay)
        if mode == "409":
            self.send_json({"detail": {"code": "erro_codex_opcoes", "params": {},
                                       "msg": "Não foi possível salvar as opções do Codex."}}, 409)
        else:
            self.reply(mode, 0, result)


server = BASE["ThreadingHTTPServer"](("127.0.0.1", int(os.environ.get("TASK42_PORT", "0"))), Handler)
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
