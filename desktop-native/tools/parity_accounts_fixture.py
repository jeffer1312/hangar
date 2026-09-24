"""Contas e modelos SINTÉTICOS para a prova da Task 12 R5 (carregado por parity_session_fixture.py).

Nenhuma conta, chave, cookie ou cota aqui é de verdade, e nada sai deste processo.
GET /api/credenciais[?forcar=true], GET /api/engines e PUT /api/credenciais/apelido respondem daqui.
GET /control/r5?list=<ok|500|empty|drop>&list_delay=<s>&engines=<ok|500|broken>&engines_delay=<s>
&rename=<ok|409|500|drop>&rename_delay=<s> muda como as próximas respostas saem (só as chaves dadas mudam).

R5b (contas Claude), também daqui, todas contra o estado em memória deste processo:
POST /api/conta-estado/{label}/login, GET .../login/passo, POST .../login/codigo {codigo}, POST .../login/cancelar;
POST /api/claude-configs/{label}/logout; POST /api/claude-configs {nome}; DELETE /api/claude-configs/{label},
/api/engines/{nome}, /api/credenciais/kimi/{nome}, /api/codex-contas/{id}.
/control/r5 também aceita login=<ok|409|500|drop>&login_delay=<s>, url_after=<s> (quando o link aparece),
step=<ok|409|500|auto> (auto: a autorização volta sozinha pelo navegador no 3º passo), code=<ok|409|504|drop|refused>
&code_delay=<s>, cancel=<ok|500|drop>&cancel_delay=<s>, logout=<ok|409|500|drop>&logout_delay=<s>,
delete=<ok|409|500|drop>&delete_delay=<s>, create=<ok|409|422|drop>&create_delay=<s>, base=<ok|out>
(out: a conta padrão, pessoal, aparece sem login). Qualquer código serve, menos "errado".

R5c1 (modelos, chaves e cookie), também contra o estado em memória: POST /api/engines/modelos ({nome} ou {base_url, api_key};
nenhum provedor é chamado: a lista é sintética), PUT /api/engines/{nome} (com a herança do backend: campo ausente ou null
fica, "" limpa, chave vazia ou mascarada mantém a atual), POST /api/credenciais/sincronizar {id} e PUT /api/credenciais/cookie.
/control/r5 também aceita probe=<ok|empty|502|drop>&probe_delay=<s>, put=<ok|400|drop>&put_delay=<s>,
sync=<ok|500|drop>&sync_delay=<s>, cookie=<ok|500|drop>&cookie_delay=<s>. Chave e cookie nunca vão para o registro
(só o tamanho) e ficam aqui só mascarados.
"""

import json
import re
import threading
import time
from urllib.parse import unquote

LOCK = threading.Lock()
MODE = {"list": "ok", "list_delay": 0.0, "engines": "ok", "engines_delay": 0.0, "rename": "ok", "rename_delay": 0.0,
        "login": "ok", "login_delay": 0.0, "url_after": 1.0, "step": "ok", "code": "ok", "code_delay": 0.0,
        "cancel": "ok", "cancel_delay": 0.0, "logout": "ok", "logout_delay": 0.0, "delete": "ok", "delete_delay": 0.0,
        "create": "ok", "create_delay": 0.0, "base": "ok", "probe": "ok", "probe_delay": 0.0, "put": "ok", "put_delay": 0.0,
        "sync": "ok", "sync_delay": 0.0, "cookie": "ok", "cookie_delay": 0.0}
# Credenciais com o cookie do painel guardado.
COOKIES = set()
ALIASES = {}
READS = {"n": 0}
# O que a prova já fez: login por rótulo, credenciais removidas, contas criadas.
LOGINS = {}
REMOVED = set()
CREATED = []
ENGINES_REMOVED = set()
# Tentativa de login em voo, por rótulo: quando começou e quantos passos já foram lidos.
ATTEMPTS = {}


def _accounts(now):
    day = 86_400
    return [
        {"id": "claude:/home/prova/.claude", "tipo": "claude", "auth_method": "oauth", "nome_natural": "pessoal", "ativa": True,
         "path": "/home/prova/.claude", "usos": [],
         "login": {"estado": "ok", "loggedIn": True, "email": "pessoal@exemplo.test", "plano": "max", "refreshExpiresAt": now + 21 * day - 60},
         "cota": {"estado": "lida", "janelas": [{"rotulo": "5h", "pct": 11 + READS["n"], "reset_ts": now + 80 * 60},
                                                  {"rotulo": "7d", "pct": 81, "reset_ts": now + 3 * day + 5 * 3600}], "idade_s": 40}},
        {"id": "claude:/home/prova/.claude-trabalho", "tipo": "claude", "auth_method": "oauth", "nome_natural": "trabalho",
         "path": "/home/prova/.claude-trabalho", "usos": [],
         "login": {"estado": "ok", "loggedIn": True, "email": "trabalho@exemplo.test", "plano": "pro", "refreshExpiresAt": now + 2 * day - 60},
         "cota": {"estado": "lida", "janelas": [{"rotulo": "5h", "pct": 34, "reset_ts": now + 3 * 3600},
                                                  {"rotulo": "7d", "pct": 40, "reset_ts": now + 5 * day}], "idade_s": 40}},
        {"id": "claude:/home/prova/.claude-reserva", "tipo": "claude", "auth_method": "oauth", "nome_natural": "reserva",
         "path": "/home/prova/.claude-reserva", "usos": [], "login": {"estado": "ok", "loggedIn": False},
         "cota": {"estado": "sem_credencial", "janelas": []}},
        {"id": "codex:/home/prova/.codex-pessoal", "tipo": "codex", "auth_method": "oauth", "codex_account": "pessoal", "codex_sync": "ready",
         "nome_natural": "codex-pessoal", "path": "/home/prova/.codex-pessoal", "usos": ["codex_cli"],
         "login": {"estado": "ok", "loggedIn": True, "email": "codex@exemplo.test", "plano": "plus"},
         "cota": {"estado": "lida", "janelas": [{"rotulo": "5h", "pct": 98, "reset_ts": now + 50 * 60},
                                                  {"rotulo": "7d", "pct": 100, "reset_ts": now + 2 * day}], "idade_s": 40,
                  "reset_credits": {"available_count": 1, "credits": [{"id": "c1", "expires_at": now + 9 * day, "status": "available"}]}}},
        {"id": "chave:kimi-coding", "tipo": "chave", "auth_method": "api_key", "nome_natural": "kimi-coding", "usos": ["claude_code"],
         "base_url": "https://api.kimi.com/coding", "chave_mascarada": "sk-kimi••••0000",
         "cota": {"estado": "lida", "janelas": [{"rotulo": "7d", "pct": 12, "reset_ts": now + 4 * day}], "idade_s": 40}},
        {"id": "kimi:kimi-coding", "tipo": "chave", "auth_method": "api_key", "nome_natural": "kimi-coding", "usos": ["kimi_cli"],
         "cota": {"estado": "lida", "janelas": [{"rotulo": "7d", "pct": 12}], "idade_s": 40}},
        {"id": "chave:deepseek", "tipo": "chave", "auth_method": "api_key", "nome_natural": "deepseek", "usos": ["claude_code"],
         "base_url": "https://api.deepseek.com/anthropic", "chave_mascarada": "sk-dee••••1111"},
        {"id": "kimi:kimi-cli", "tipo": "chave", "auth_method": "api_key", "nome_natural": "kimi-cli", "usos": ["kimi_cli"], "gerenciada": False,
         "cota": {"estado": "lida", "janelas": [{"rotulo": "5h", "pct": 20, "reset_ts": now + 2 * 3600}], "idade_s": 1500}},
        {"id": "chave:opencode-zen", "tipo": "chave", "auth_method": "api_key", "nome_natural": "opencode-zen", "usos": [],
         "base_url": "https://opencode.ai/zen", "chave_mascarada": "sk-ope••••2222", "aceita_cookie": True, "cookie_definido": False},
    ]


ENGINES = {
    "kimi-coding": {"label": "kimi-coding", "base_url": "https://api.kimi.com/coding", "model": "k3-256k", "context_window": 256000,
                    "adaptive_thinking": True, "api_key": "sk-kimi••••0000", "api_key_definida": True},
    "deepseek": {"label": "deepseek", "base_url": "https://api.deepseek.com/anthropic", "model": "deepseek-v4", "context_window": 128000,
                 "subagent_model": "deepseek-v4-flash", "adaptive_thinking": False, "api_key": "sk-dee••••1111", "api_key_definida": True},
}


def _state(now):
    """A lista sintética com o que a prova já fez: sem as removidas, com as criadas e o login atual de cada uma."""
    rows = [r for r in _accounts(now) if r["id"] not in REMOVED]
    known = {r["id"] for r in rows}
    for name, engine in ENGINES.items():
        if f"chave:{name}" not in known and name not in ENGINES_REMOVED:
            rows.append({"id": f"chave:{name}", "tipo": "chave", "auth_method": "api_key", "nome_natural": name, "usos": ["claude_code"],
                         "base_url": engine["base_url"], "chave_mascarada": engine.get("api_key") or None})
    for row in rows:
        if row.get("aceita_cookie"):
            row["cookie_definido"] = row["id"] in COOKIES
            if row["cookie_definido"]:
                row["cota"] = {"estado": "lida", "janelas": [{"rotulo": "5h", "pct": 27, "reset_ts": now + 2 * 3600},
                                                             {"rotulo": "7d", "pct": 44, "reset_ts": now + 4 * 86_400}], "idade_s": 0}
    for name in CREATED:
        rows.append({"id": f"claude:/home/prova/.claude-{name}", "tipo": "claude", "auth_method": "oauth", "nome_natural": name,
                     "path": f"/home/prova/.claude-{name}", "usos": [], "login": {"estado": "ok", "loggedIn": False},
                     "cota": {"estado": "sem_credencial", "janelas": []}})
    for row in rows:
        label = row["nome_natural"]
        if row["tipo"] != "claude":
            continue
        logged = LOGINS.get(label, None if not (label == "pessoal" and MODE["base"] == "out") else False)
        if logged is True:
            row["login"] = {"estado": "ok", "loggedIn": True, "email": f"{label}@exemplo.test", "plano": "max",
                            "refreshExpiresAt": now + 30 * 86_400}
            row["cota"] = {"estado": "lida", "janelas": [{"rotulo": "5h", "pct": 3, "reset_ts": now + 4 * 3600},
                                                         {"rotulo": "7d", "pct": 9, "reset_ts": now + 6 * 86_400}], "idade_s": 0}
        elif logged is False:
            row["login"], row["cota"] = {"estado": "ok", "loggedIn": False}, {"estado": "sem_credencial", "janelas": []}
    return rows


def listing(forced):
    now = time.time()
    with LOCK:
        if forced:
            READS["n"] += 1
        rows = _state(now)
        for row in rows:
            row["apelido"] = ALIASES.get(row["id"])
            row["nome"] = row["apelido"] or row["nome_natural"]
            if forced and row.get("cota", {}).get("idade_s") is not None:
                row["cota"]["idade_s"] = 0
        return rows


def control(query):
    with LOCK:
        for key, raw in ((k, v[0]) for k, v in query.items()):
            if key in MODE:
                MODE[key] = float(raw) if key.endswith("_delay") or key == "url_after" else raw
        return dict(MODE)


def drop(handler):
    handler.close_connection = True
    handler.connection.shutdown(2)


def handle_get(handler, path, query):
    """True quando a rota é de contas e já foi respondida."""
    if path == "/api/credenciais":
        with LOCK:
            mode, delay = MODE["list"], MODE["list_delay"]
        time.sleep(delay)
        if mode == "drop":
            drop(handler)
        elif mode == "500":
            handler.send_json({"detail": {"code": "erro_sintetico", "params": {}, "msg": "leitura das contas falhou (sintético)"}}, 500)
        else:
            handler.send_json([] if mode == "empty" else listing(query.get("forcar", ["false"])[0] == "true"))
        return True
    if path == "/api/engines":
        with LOCK:
            mode, delay = MODE["engines"], MODE["engines_delay"]
        time.sleep(delay)
        if mode == "500":
            handler.send_json({"detail": "motores ilegíveis (sintético)"}, 500)
        else:
            broken = mode == "broken"
            with LOCK:
                engines = {k: v for k, v in ENGINES.items() if k not in ENGINES_REMOVED}
            handler.send_json({"motores": {} if broken else engines, "arquivo_corrompido": broken,
                               "arquivo_caminho": "/home/prova/.claude/engines.json"})
        return True
    parts = [unquote(p) for p in path.strip("/").split("/")]
    if parts[:2] == ["api", "conta-estado"] and parts[3:] == ["login", "passo"]:
        step(handler, parts[2])
        return True
    return False


def _labels():
    return {r["nome_natural"] for r in _state(time.time()) if r["tipo"] == "claude"}


def _fail(handler, status, msg):
    handler.send_json({"detail": {"code": "erro_sintetico", "params": {}, "msg": msg}}, status)


def _refused(handler, key, msg):
    """Atraso e recusa do modo da ação; True quando já respondeu com a recusa."""
    with LOCK:
        mode, delay = MODE[key], MODE.get(f"{key}_delay", 0.0)
    time.sleep(delay)
    if mode in ("409", "422", "500", "504"):
        _fail(handler, int(mode), msg)
        return True
    return False


def step(handler, label):
    with LOCK:
        mode, attempt = MODE["step"], ATTEMPTS.get(label)
        if attempt is not None:
            attempt["reads"] += 1
        done = mode == "auto" and attempt is not None and attempt["reads"] >= 3
        if done:
            LOGINS[label] = True
            ATTEMPTS.pop(label, None)
    if mode in ("409", "500"):
        _fail(handler, int(mode), "a tentativa de login acabou no servidor (sintético)")
    elif attempt is None:
        _fail(handler, 409, "sem tentativa de login em curso (sintético)")
    elif done:
        handler.send_json({"etapa": "concluido", "email": f"{label}@exemplo.test", "plano": "max"})
    elif time.time() - attempt["t"] >= MODE["url_after"]:
        handler.send_json({"etapa": "aguardando", "url": f"https://claude.exemplo.test/oauth/authorize?conta={label}&prova=1"})
    else:
        handler.send_json({"etapa": "aguardando"})


def _login(handler, label, action, body, log):
    if action == "":
        if _refused(handler, "login", "já há uma tentativa de login nesta conta (sintético)"):
            return
        if label not in _labels():
            _fail(handler, 404, f"conta {label} não existe")
            return
        with LOCK:
            ATTEMPTS[label] = {"t": time.time(), "reads": 0}
        drop(handler) if MODE["login"] == "drop" else handler.send_json({"ok": True})
    elif action == "codigo":
        code = (body or {}).get("codigo", "")
        log["code_len"] = len(code)
        if _refused(handler, "code", "o código não foi aceito (sintético)"):
            return
        with LOCK:
            alive = label in ATTEMPTS
        if not alive:
            _fail(handler, 409, "sem tentativa de login em curso (sintético)")
        elif MODE["code"] == "refused" or code.strip() == "errado":
            handler.send_json({"ok": False})
        else:
            with LOCK:
                LOGINS[label] = True
                ATTEMPTS.pop(label, None)
            if MODE["code"] == "drop":
                drop(handler)
            else:
                handler.send_json({"ok": True, "email": f"{label}@exemplo.test", "plano": "max"})
    elif action == "cancelar":
        if _refused(handler, "cancel", "não consegui fechar a janela do login (sintético)"):
            return
        with LOCK:
            ATTEMPTS.pop(label, None)
        drop(handler) if MODE["cancel"] == "drop" else handler.send_json({"ok": True})


def handle_post(handler, path, body):
    """Escritas das contas Claude (R5b); True quando a rota é daqui e já foi respondida."""
    parts = [unquote(p) for p in path.strip("/").split("/")]
    log = {"post": path}
    if parts[:2] == ["api", "conta-estado"] and len(parts) >= 4 and parts[3] == "login" \
            and "/".join(parts[4:]) in ("", "codigo", "cancelar"):
        _login(handler, parts[2], "/".join(parts[4:]), body, log)
    elif parts[:2] == ["api", "claude-configs"] and parts[3:] == ["logout"]:
        label = parts[2]
        if not _refused(handler, "logout", "a conta não saiu: o logout não foi confirmado (sintético)"):
            if label not in _labels():
                _fail(handler, 404, f"conta {label} não existe")
            else:
                with LOCK:
                    LOGINS[label] = False
                drop(handler) if MODE["logout"] == "drop" else handler.send_json({"ok": True})
    elif parts == ["api", "claude-configs"]:
        name = (body or {}).get("nome", "")
        log["nome"] = name
        if not _refused(handler, "create", "não consegui criar a pasta da conta (sintético)"):
            if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,31}", name):
                handler.send_json({"detail": [{"msg": "String should match pattern '^[a-z0-9][a-z0-9_-]{0,31}'"}]}, 422)
            elif name in _labels():
                _fail(handler, 409, f"a conta {name} já existe")
            else:
                with LOCK:
                    CREATED.append(name)
                if MODE["create"] == "drop":
                    drop(handler)
                else:
                    handler.send_json({"path": f"/home/prova/.claude-{name}", "label": name, "active": False})
    elif parts == ["api", "engines", "modelos"]:
        _probe(handler, body or {}, log)
    elif parts == ["api", "credenciais", "sincronizar"]:
        _sync(handler, (body or {}).get("id", ""), log)
    else:
        return False
    print("ACCOUNTS", json.dumps(log), flush=True)
    return True


def handle_delete(handler, path):
    parts = [unquote(p) for p in path.strip("/").split("/")]
    kind, name = "/".join(parts[1:-1]), parts[-1]
    ids = {"claude-configs": "claude:/home/prova/.claude" + ("" if name == "pessoal" else f"-{name}"),
           "engines": f"chave:{name}", "credenciais/kimi": f"kimi:{name}", "codex-contas": f"codex:/home/prova/.codex-{name}"}
    if parts[0] != "api" or kind not in ids:
        return False
    if not _refused(handler, "delete", "a conta está em uso por uma sessão (sintético)"):
        target = ids[kind]
        with LOCK:
            known = any(r["id"] == target for r in _state(time.time()))
            if known and name in CREATED and kind == "claude-configs":
                CREATED.remove(name)
            elif known:
                REMOVED.add(target)
                if kind == "engines":
                    ENGINES_REMOVED.add(name)
        if not known:
            _fail(handler, 404, f"{name} não existe")
        else:
            drop(handler) if MODE["delete"] == "drop" else handler.send_json({"ok": True})
    print("ACCOUNTS", json.dumps({"delete": path}), flush=True)
    return True


def _mask(key):
    return f"{key[:4]}••••{key[-4:]}" if len(key) > 8 else "••••"


def _probe(handler, body, log):
    """Testar e listar: nenhum provedor é chamado. Com `nome`, vale o motor salvo; com endereço e chave, os digitados."""
    log.update({"nome": body.get("nome"), "base_url": body.get("base_url"), "key_len": len(body.get("api_key") or "")})
    with LOCK:
        mode, delay = MODE["probe"], MODE["probe_delay"]
        saved = ENGINES.get(body.get("nome") or "")
    time.sleep(delay)
    if body.get("nome") and (body.get("base_url") or body.get("api_key")):
        _fail(handler, 400, "nome já usa o motor salvo; não envie base_url/api_key junto")
    elif body.get("nome") and not saved:
        _fail(handler, 404, "motor nao encontrado")
    elif not body.get("nome") and not (body.get("base_url") and body.get("api_key")):
        _fail(handler, 400, "informe nome de um motor salvo, ou base_url + api_key")
    elif mode == "drop":
        drop(handler)
    elif mode == "502":
        handler.send_json({"detail": "401 Unauthorized: chave recusada pelo provedor (sintético)"}, 502)
    else:
        url = (saved or {}).get("base_url") or body.get("base_url") or ""
        models = [] if mode == "empty" else (
            [{"id": "k3-256k", "context_length": 256000, "vision": True}, {"id": "k2.7-turbo", "context_length": 131072, "vision": False},
             {"id": "k3-mini", "context_length": None, "vision": None}] if "kimi" in url else
            [{"id": "prova-grande", "context_length": 1000000, "vision": True}, {"id": "prova-rapido", "context_length": 200000, "vision": True},
             {"id": "prova-texto", "context_length": 64000, "vision": False}])
        handler.send_json({"modelos": models})


def _sync(handler, cred, log):
    log["id"] = cred
    with LOCK:
        mode, delay = MODE["sync"], MODE["sync_delay"]
        exists = cred.startswith("chave:") and cred[len("chave:"):] in ENGINES and cred[len("chave:"):] not in ENGINES_REMOVED
    time.sleep(delay)
    if not exists:
        _fail(handler, 404, f"credencial {cred} não existe")
    elif mode == "500":
        _fail(handler, 500, "o Pi recusou a gravação (sintético)")
    elif mode == "drop":
        drop(handler)
    else:
        var = "HANGAR_" + cred[len("chave:"):].upper().replace("-", "_") + "_KEY"
        handler.send_json({"id": cred, "modelos": 3, "resultado": {
            "pi": {"ok": True, "motivo": ""}, "kimi": {"ok": False, "motivo": "nao-instalado"},
            "codex": {"ok": True, "motivo": f"exporte {var} no shell para o Codex usar a chave"}}})


def _put_engine(handler, name, body, log):
    log.update({"engine": name, "key_len": len((body or {}).get("api_key") or ""), "fields": sorted((body or {}).keys())})
    with LOCK:
        mode, delay = MODE["put"], MODE["put_delay"]
    time.sleep(delay)
    if mode == "400":
        handler.send_json({"detail": "base_url: endereço público precisa ser https (sintético)"}, 400)
        return
    if not re.fullmatch(r"[a-z0-9_-]{1,32}", name) or not isinstance(body, dict):
        handler.send_json({"detail": "nome do motor inválido"}, 400)
        return
    with LOCK:
        current = dict(ENGINES.get(name, {})) if name not in ENGINES_REMOVED else {}
        sent = body.get("api_key")
        body = dict(body)
        if current.get("api_key") and (not isinstance(sent, str) or not sent.strip() or sent.strip() == current["api_key"]):
            body["api_key"] = current["api_key"]
        elif isinstance(sent, str) and sent.strip():
            body["api_key"] = _mask(sent.strip())
        for field, value in current.items():
            if field not in body or body[field] is None:
                body[field] = value
        saved = {k: v for k, v in body.items() if v != ""}
        saved["api_key_definida"] = bool(saved.get("api_key"))
        ENGINES[name] = saved
        ENGINES_REMOVED.discard(name)
        REMOVED.discard(f"chave:{name}")
        engines = {k: v for k, v in ENGINES.items() if k not in ENGINES_REMOVED}
    if mode == "drop":
        drop(handler)
    else:
        handler.send_json({"motores": engines})


def _put_cookie(handler, body, log):
    cred, ws, cookie = (body or {}).get("id", ""), (body or {}).get("workspace_id", ""), (body or {}).get("auth_cookie", "")
    log.update({"id": cred, "ws_len": len(ws), "cookie_len": len(cookie)})
    with LOCK:
        mode, delay = MODE["cookie"], MODE["cookie_delay"]
    time.sleep(delay)
    if mode == "500":
        _fail(handler, 500, "não consegui gravar o cookie (sintético)")
        return
    with LOCK:
        if ws.strip() and cookie.strip():
            COOKIES.add(cred)
        else:
            COOKIES.discard(cred)
        kept = cred in COOKIES
    drop(handler) if mode == "drop" else handler.send_json({"id": cred, "cookie_definido": kept})


def handle_put(handler, path, body):
    parts = [unquote(p) for p in path.strip("/").split("/")]
    if parts[:2] == ["api", "engines"] and len(parts) == 3 or path == "/api/credenciais/cookie":
        log = {"put": path}
        if path == "/api/credenciais/cookie":
            _put_cookie(handler, body, log)
        else:
            _put_engine(handler, parts[2], body, log)
        print("ACCOUNTS", json.dumps(log), flush=True)
        return True
    if path != "/api/credenciais/apelido":
        return False
    with LOCK:
        mode, delay = MODE["rename"], MODE["rename_delay"]
    time.sleep(delay)
    alias = (body or {}).get("apelido", "")
    if mode == "drop":
        # Pedido aplicado, resposta perdida: o app não sabe se gravou.
        with LOCK:
            ALIASES[body["id"]] = alias.strip() or None
        drop(handler)
    elif mode in ("409", "500"):
        handler.send_json({"detail": {"code": "erro_sintetico", "params": {}, "msg": "apelido recusado (sintético)"}}, int(mode))
    elif len(alias) > 40:
        handler.send_json({"detail": [{"msg": "String should have at most 40 characters"}]}, 422)
    else:
        with LOCK:
            ALIASES[body["id"]] = alias.strip() or None
            saved = ALIASES[body["id"]]
        handler.send_json({"id": body["id"], "apelido": saved})
    print("ACCOUNTS", json.dumps({"put": path, "mode": mode}), flush=True)
    return True
