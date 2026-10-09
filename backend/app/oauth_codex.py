"""Login OAuth do ChatGPT (o do Codex) feito pelo app, uma vez, e espalhado pros CLIs que o aceitam.

Codex CLI, Pi e omp fazem o MESMO login: mesmo `client_id`, mesmo endpoint de refresh, mesmo
fluxo de código de dispositivo. Cada um guarda o resultado no formato dele — `~/.codex/auth.json`,
`~/.pi/agent/auth.json` e a tabela `auth_credentials` do `~/.omp/agent/agent.db`. Sem isto a
pessoa loga três vezes na mesma conta.

O fluxo de dispositivo é do Rust, que guarda o resultado no cofre
(`~/.hangar/auth/openai-codex.json`, 0600) e escreve nos três stores; sem ele não há fluxo
Python. Daqui sai a leitura do cofre e dos stores que a saúde dos harnesses usa, e a
importação/propagação que ela pede (delegada ao Rust quando ele está de pé). Depois disso cada CLI renova o token por conta própria: medido, o refresh rotaciona
mas o refresh anterior continua válido, então cópias independentes coexistem — o mesmo que já
acontece hoje com Codex e Pi logados em separado.

Regra de escrita nos stores: só ENTRA onde não há login; um login que já existe lá é da pessoa e
não é trocado (mesma regra do `_gravar_auth_pi` pra credencial oauth).
"""
from __future__ import annotations

import base64
import json
import os
import sqlite3
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from app import atomico
from app.agentes_sync import _codex_dir, _pi_dir

_JWT_CLAIM = "https://api.openai.com/auth"
PROVEDOR = "openai-codex"


def cofre() -> Path:
    return Path.home() / ".hangar" / "auth" / "openai-codex.json"


def _omp_db(home: Path | None) -> Path:
    from app import omp_dirs
    # `home` explícito é semente de teste: raiz fixa, sem perfil nem variável de ambiente.
    # Estrito: aqui se GRAVA credencial; perfil inválido tem que falhar, não cair na raiz errada.
    return omp_dirs.agent_dir(home=home, env={} if home else None, estrito=True) / "agent.db"


# ---------------------------------------------------------------- tokens

def _claims(access: str) -> dict:
    try:
        parte = access.split(".")[1]
        parte += "=" * (-len(parte) % 4)
        return json.loads(base64.urlsafe_b64decode(parte))
    except (IndexError, ValueError):
        return {}


@dataclass
class Tokens:
    access: str
    refresh: str
    id_token: str
    expires_ms: int
    account_id: str
    plano: str = ""

    @classmethod
    def de_resposta(cls, r: dict) -> "Tokens":
        claims = _claims(r["access_token"])
        auth = claims.get(_JWT_CLAIM) or {}
        exp = claims.get("exp")
        expires = int(exp) * 1000 if exp else int(time.time() + r.get("expires_in", 0)) * 1000
        return cls(access=r["access_token"], refresh=r["refresh_token"], id_token=r.get("id_token", ""),
                   expires_ms=expires, account_id=auth.get("chatgpt_account_id", ""),
                   plano=auth.get("chatgpt_plan_type", ""))

    def para_pi(self) -> dict:
        return {"type": "oauth", "access": self.access, "refresh": self.refresh,
                "expires": self.expires_ms, "accountId": self.account_id}


def _gravar_json(alvo: Path, dados: dict) -> None:
    """tmp+rename, e o tmp já NASCE 0600: token em arquivo 0644 por um instante é vazamento."""
    tmp = alvo.with_name(f"{alvo.name}.{os.getpid()}.hangar.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        fh.write(json.dumps(dados, indent=2) + "\n")
    atomico.substituir(tmp, alvo)


def salvar_cofre(t: Tokens) -> None:
    alvo = cofre()
    alvo.parent.mkdir(parents=True, exist_ok=True)
    _gravar_json(alvo, t.__dict__)


def importar_do_codex(home: Path | None = None) -> Tokens | None:
    """Quem já logou pelo `codex login` não precisa logar de novo: o cofre nasce do auth.json dele."""
    _, managed = _managed_device("import")
    if managed:
        return ler_cofre()
    try:
        d = json.loads((_codex_dir(home) / "auth.json").read_text(encoding="utf-8"))
        tk = d["tokens"]
        t = Tokens.de_resposta({"access_token": tk["access_token"], "refresh_token": tk["refresh_token"],
                                "id_token": tk.get("id_token", "")})
    except (OSError, ValueError, KeyError, TypeError):
        return None
    salvar_cofre(t)
    return t


def ler_cofre() -> Tokens | None:
    try:
        d = json.loads(cofre().read_text(encoding="utf-8"))
        return Tokens(**{k: d[k] for k in Tokens.__dataclass_fields__ if k in d})
    except (OSError, ValueError, TypeError):
        return None


# ---------------------------------------------------------------- stores dos CLIs

def _codex_tem_login(home: Path | None) -> bool:
    try:
        d = json.loads((_codex_dir(home) / "auth.json").read_text(encoding="utf-8"))
        return bool((d.get("tokens") or {}).get("refresh_token"))
    except (OSError, ValueError, AttributeError):
        return False


def _pi_tem_login(home: Path | None) -> bool:
    try:
        d = json.loads((_pi_dir(home) / "auth.json").read_text(encoding="utf-8"))
        e = d.get(PROVEDOR)
        return isinstance(e, dict) and e.get("type") == "oauth" and bool(e.get("refresh"))
    except (OSError, ValueError, AttributeError):
        return False


def _omp_tem_login(home: Path | None) -> bool:
    db = _omp_db(home)
    if not db.is_file():
        return False
    try:
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        try:
            n = con.execute("select count(*) from auth_credentials where provider=? and credential_type='oauth'",
                            (PROVEDOR,)).fetchone()[0]
        finally:
            con.close()
        return n > 0
    except sqlite3.Error:
        return False


def _para_codex(t: Tokens, home: Path | None) -> tuple[bool, str]:
    _require_python_writer()
    d = _codex_dir(home)
    if not d.is_dir():
        return False, "nao-instalado"
    if _codex_tem_login(home):
        return True, "ja-logado"
    alvo = d / "auth.json"
    _gravar_json(alvo, {
        "auth_mode": "chatgpt", "OPENAI_API_KEY": None,
        "tokens": {"id_token": t.id_token, "access_token": t.access,
                   "refresh_token": t.refresh, "account_id": t.account_id},
        "last_refresh": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    })
    return True, str(alvo)


def _para_pi(t: Tokens, home: Path | None) -> tuple[bool, str]:
    d = _pi_dir(home)
    if not d.is_dir():
        return False, "nao-instalado"
    if _pi_tem_login(home):
        return True, "ja-logado"
    alvo = d / "auth.json"
    dados: dict[str, Any] = {}
    if alvo.exists():
        try:
            dados = json.loads(alvo.read_text(encoding="utf-8"))
        except ValueError:
            return False, "auth-invalido"
        if not isinstance(dados, dict):
            return False, "auth-invalido"
    dados[PROVEDOR] = t.para_pi()
    _gravar_json(alvo, dados)
    return True, str(alvo)


def _para_omp(t: Tokens, home: Path | None) -> tuple[bool, str]:
    db = _omp_db(home)
    if not db.is_file():
        return False, "nao-instalado"
    if _omp_tem_login(home):
        return True, "ja-logado"
    # O omp é fork do Pi: o `data` é a credencial do Pi sem o `type`, que mora na coluna.
    dados = {k: v for k, v in t.para_pi().items() if k != "type"}
    try:
        con = sqlite3.connect(db, timeout=5)
        try:
            con.execute("insert into auth_credentials (provider, credential_type, data, identity_key) "
                        "values (?, 'oauth', ?, ?)", (PROVEDOR, json.dumps(dados), t.account_id or None))
            con.commit()
        finally:
            con.close()
    except sqlite3.Error as e:
        return False, f"sqlite: {e}"
    return True, str(db)


def propagar(t: Tokens | None = None, home: Path | None = None) -> dict[str, dict]:
    response, managed = _managed_device("propagate")
    if managed:
        return response
    t = t or ler_cofre()
    if t is None:
        return {a: {"ok": False, "motivo": "sem-login"} for a in ("codex", "pi", "omp")}
    saida = {}
    for nome, fn in (("codex", _para_codex), ("pi", _para_pi), ("omp", _para_omp)):
        try:
            ok, motivo = fn(t, home)
        except OSError as e:
            ok, motivo = False, str(e)
        saida[nome] = {"ok": ok, "motivo": motivo}
    return saida


def _managed_device(action):
    from app.account_bridge import request_device
    return request_device(action)


def _require_python_writer():
    from fastapi import HTTPException

    from app.account_bridge import owner_mode
    if owner_mode() != "python":
        raise HTTPException(503, detail={"code": "account_device_python_writer_disabled"})
