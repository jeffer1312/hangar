"""Contas e modelos SINTÉTICOS para a prova da Task 12 R5 (carregado por parity_session_fixture.py).

Nenhuma conta, chave, cookie ou cota aqui é de verdade, e nada sai deste processo.
GET /api/credenciais[?forcar=true], GET /api/engines e PUT /api/credenciais/apelido respondem daqui.
GET /control/r5?list=<ok|500|empty|drop>&list_delay=<s>&engines=<ok|500|broken>&engines_delay=<s>
&rename=<ok|409|500|drop>&rename_delay=<s> muda como as próximas respostas saem (só as chaves dadas mudam).
"""

import json
import threading
import time

LOCK = threading.Lock()
MODE = {"list": "ok", "list_delay": 0.0, "engines": "ok", "engines_delay": 0.0, "rename": "ok", "rename_delay": 0.0}
ALIASES = {}
READS = {"n": 0}


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


def listing(forced):
    now = time.time()
    with LOCK:
        if forced:
            READS["n"] += 1
        rows = _accounts(now)
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
                MODE[key] = float(raw) if key.endswith("_delay") else raw
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
            handler.send_json({"motores": {} if broken else ENGINES, "arquivo_corrompido": broken,
                               "arquivo_caminho": "/home/prova/.claude/engines.json"})
        return True
    return False


def handle_put(handler, path, body):
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
