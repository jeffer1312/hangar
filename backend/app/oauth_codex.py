"""Login OAuth do ChatGPT (o do Codex) feito pelo app, uma vez, e espalhado pros CLIs que o aceitam.

Codex CLI, Pi e omp fazem o MESMO login: mesmo `client_id`, mesmo endpoint de refresh, mesmo
fluxo de código de dispositivo. Cada um guarda o resultado no formato dele — `~/.codex/auth.json`,
`~/.pi/agent/auth.json` e a tabela `auth_credentials` do `~/.omp/agent/agent.db`. Sem isto a
pessoa loga três vezes na mesma conta.

O app roda o fluxo de dispositivo sozinho (stdlib, sem depender de nenhum dos três CLIs estar
instalado), guarda o resultado no cofre (`~/.hangar/auth/openai-codex.json`, 0600) e escreve nos
três stores. Depois disso cada CLI renova o token por conta própria: medido, o refresh rotaciona
mas o refresh anterior continua válido, então cópias independentes coexistem — o mesmo que já
acontece hoje com Codex e Pi logados em separado.

Regra de escrita nos stores: só ENTRA onde não há login; um login que já existe lá é da pessoa e
não é trocado (mesma regra do `_gravar_auth_pi` pra credencial oauth).
"""
from __future__ import annotations

import base64
import json
import logging
import os
import sqlite3
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from app import atomico
from app.agentes_sync import _codex_dir, _pi_dir

_log = logging.getLogger("hangar.oauth_codex")

CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"
_AUTH = "https://auth.openai.com"
_TOKEN_URL = f"{_AUTH}/oauth/token"
_DEVICE_USERCODE_URL = f"{_AUTH}/api/accounts/deviceauth/usercode"
_DEVICE_TOKEN_URL = f"{_AUTH}/api/accounts/deviceauth/token"
_DEVICE_REDIRECT_URI = f"{_AUTH}/deviceauth/callback"
VERIFICATION_URL = f"{_AUTH}/codex/device"
_JWT_CLAIM = "https://api.openai.com/auth"
_TIMEOUT_S = 15 * 60
PROVEDOR = "openai-codex"


def cofre() -> Path:
    return Path.home() / ".hangar" / "auth" / "openai-codex.json"


def _omp_db(home: Path | None) -> Path:
    from app import omp_dirs
    # `home` explícito é semente de teste: raiz fixa, sem perfil nem variável de ambiente.
    # Estrito: aqui se GRAVA credencial; perfil inválido tem que falhar, não cair na raiz errada.
    return omp_dirs.agent_dir(home=home, env={} if home else None, estrito=True) / "agent.db"


# ---------------------------------------------------------------- HTTP (seam de teste)

def _http(url: str, corpo: dict, *, form: bool) -> tuple[int, dict]:
    if form:
        dados = urllib.parse.urlencode(corpo).encode()
        tipo = "application/x-www-form-urlencoded"
    else:
        dados = json.dumps(corpo).encode()
        tipo = "application/json"
    # A Cloudflare na frente do auth.openai.com devolve 530 pro User-Agent padrão do urllib
    # (medido); qualquer outro passa.
    req = urllib.request.Request(url, data=dados, headers={"Content-Type": tipo, "User-Agent": "hangar/1.0"})
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            corpo = r.read()
        try:
            return r.status, json.loads(corpo or b"{}")
        except ValueError:
            return r.status, {}
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read() or b"{}")
        except ValueError:
            return e.code, {}


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


def estado(home: Path | None = None) -> dict:
    response, managed = _managed_device("state")
    if managed:
        return response
    t = ler_cofre()
    return {
        "cofre": t is not None,
        "plano": t.plano if t else "",
        "expira_em": t.expires_ms if t else None,
        "codex": _codex_tem_login(home),
        "pi": _pi_tem_login(home),
        "omp": _omp_tem_login(home),
    }


# ---------------------------------------------------------------- fluxo de dispositivo

@dataclass
class Tentativa:
    device_auth_id: str
    user_code: str
    intervalo_s: float
    inicio: float = field(default_factory=time.monotonic)
    etapa: str = "aguardando"      # aguardando | concluido | falhou | cancelado
    erro: str = ""
    resultado: dict | None = None
    _parar: threading.Event = field(default_factory=threading.Event)


_lock = threading.Lock()
_tentativa: Tentativa | None = None


def _trocar_codigo(codigo: str, verificador: str) -> Tokens:
    st, r = _http(_TOKEN_URL, {
        "grant_type": "authorization_code", "client_id": CLIENT_ID, "code": codigo,
        "code_verifier": verificador, "redirect_uri": _DEVICE_REDIRECT_URI,
    }, form=True)
    if st != 200 or not r.get("access_token") or not r.get("refresh_token"):
        raise RuntimeError(f"troca do código falhou ({st}): {json.dumps(r)[:200]}")
    return Tokens.de_resposta(r)


def _vigiar(t: Tentativa, home: Path | None) -> None:
    espera = max(t.intervalo_s, 1.0)
    while not t._parar.is_set():
        if time.monotonic() - t.inicio > _TIMEOUT_S:
            t.etapa, t.erro = "falhou", "tempo esgotado"
            return
        if t._parar.wait(espera):
            return
        try:
            st, r = _http(_DEVICE_TOKEN_URL, {"device_auth_id": t.device_auth_id, "user_code": t.user_code},
                          form=False)
        except OSError as e:
            _log.debug("device poll: %r", e)
            continue
        if st == 200 and r.get("authorization_code") and r.get("code_verifier"):
            try:
                tokens = _trocar_codigo(r["authorization_code"], r["code_verifier"])
                salvar_cofre(tokens)
                t.resultado = propagar(tokens, home)
                t.etapa = "concluido"
            except (RuntimeError, OSError) as e:
                t.etapa, t.erro = "falhou", str(e)
            return
        codigo = (r.get("error") or {})
        codigo = codigo.get("code") if isinstance(codigo, dict) else codigo
        if st in (403, 404) or codigo == "deviceauth_authorization_pending":
            continue
        if codigo == "slow_down":
            espera += 5
            continue
        t.etapa, t.erro = "falhou", f"{st}: {json.dumps(r)[:200]}"
        return


def iniciar(home: Path | None = None) -> dict:
    response, managed = _managed_device("start")
    if managed:
        return response
    global _tentativa
    with _lock:
        if _tentativa and _tentativa.etapa == "aguardando":
            raise RuntimeError("login já em andamento")
        try:
            st, r = _http(_DEVICE_USERCODE_URL, {"client_id": CLIENT_ID}, form=False)
        except OSError as e:
            raise RuntimeError(f"sem acesso a auth.openai.com: {e}") from e
        if st != 200 or not r.get("device_auth_id") or not r.get("user_code"):
            raise RuntimeError(f"pedido de código falhou ({st}): {json.dumps(r)[:200]}")
        intervalo = r.get("interval", 5)
        try:
            intervalo = float(str(intervalo).strip())
        except ValueError:
            intervalo = 5.0
        t = Tentativa(device_auth_id=r["device_auth_id"], user_code=r["user_code"], intervalo_s=intervalo)
        _tentativa = t
        threading.Thread(target=_vigiar, args=(t, home), daemon=True, name="oauth-codex").start()
        return passo()


def passo() -> dict:
    response, managed = _managed_device("status")
    if managed:
        return response
    t = _tentativa
    if t is None:
        return {"etapa": "idle"}
    return {"etapa": t.etapa, "user_code": t.user_code, "url": VERIFICATION_URL,
            "erro": t.erro, "resultado": t.resultado}


def cancelar() -> dict:
    response, managed = _managed_device("cancel")
    if managed:
        return response
    global _tentativa
    with _lock:
        t = _tentativa
        if t and t.etapa == "aguardando":
            t._parar.set()
            t.etapa = "cancelado"
        _tentativa = None
    return {"etapa": "idle"}


def propagate_secondary() -> dict[str, dict]:
    """Gancho privado: lê o cofre local e grava somente Pi/omp."""
    tokens = ler_cofre()
    result = {}
    for name, writer in (("pi", _para_pi), ("omp", _para_omp)):
        if tokens is None:
            result[name] = {"ok": False, "motivo": "sem-login"}
            continue
        try:
            ok, reason = writer(tokens, None)
        except (OSError, ValueError):
            ok, reason = False, "armazenamento-indisponivel"
        if not ok and reason.startswith("sqlite:"):
            reason = "sqlite-indisponivel"
        result[name] = {"ok": ok, "motivo": reason}
    return result


def _managed_device(action):
    from app.account_bridge import request_device
    return request_device(action)


def _require_python_writer():
    from fastapi import HTTPException
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is not None and getattr(coordinator, "mode", "python") != "python":
        raise HTTPException(503, detail={"code": "account_device_python_writer_disabled"})

@dataclass
class SecondaryJob:
    key: Any
    future: Any
    thread: threading.Thread | None = None
    acknowledged: bool = False


class SecondaryJobs:
    """O registro sobrevive à resposta HTTP; somente o fim do escritor publica terminal."""

    def __init__(self):
        self.lock = threading.Lock()
        self.jobs = {}
        self.closing = set()

    @staticmethod
    def snapshot(instance, operation, job):
        response = {"instance": instance, "operation_id": operation, "status": "pending"}
        if job.future.done():
            if job.thread is not None:
                job.thread.join()
            response.update(status="completed", result=job.future.result())
        return response

    def start(self, instance, operation):
        from concurrent.futures import Future
        from fastapi import HTTPException
        from app.account_lifecycle import AccountKey, Provider
        with self.lock:
            job = self.jobs.get((instance, operation))
            if job is None:
                if instance in self.closing:
                    raise HTTPException(503, detail={"code": "device_writer_closing"})
                key = AccountKey.new(Provider.CODEX, _codex_dir(None))
                if any(other.key == key and (not other.future.done() or (other.thread is not None and other.thread.is_alive()))
                       for other in self.jobs.values()):
                    raise HTTPException(503, detail={"code": "device_writer_pending"})
                job = SecondaryJob(key, Future())
                self.jobs[instance, operation] = job
                def run():
                    try:
                        result = propagate_secondary()
                    except Exception:
                        result = {name: {"ok": False, "motivo": "armazenamento-indisponivel"} for name in ("pi", "omp")}
                    job.future.set_result(result)
                thread = threading.Thread(target=run, name="device-secondary-writer", daemon=False)
                try:
                    thread.start()
                    job.thread = thread
                except Exception:
                    job.future.set_result({name: {"ok": False, "motivo": "armazenamento-indisponivel"} for name in ("pi", "omp")})
        return self.snapshot(instance, operation, job)

    def acknowledge(self, instance, operation):
        from fastapi import HTTPException
        with self.lock:
            job = self.jobs.get((instance, operation))
            if job is None or not job.future.done():
                raise HTTPException(503, detail={"code": "device_writer_pending"})
            job.acknowledged = True
        # Resultado conservado até closing: uma resposta de ack perdida não admite outro writer.
        self.snapshot(instance, operation, job)
        return {"instance": instance, "operation_id": operation, "status": "acknowledged"}

    def begin_close(self, instance):
        with self.lock:
            self.closing.add(instance)
            return [(key, job) for key, job in self.jobs.items() if key[0] == instance]

    def finish_close(self, instance, jobs):
        for (_, operation), job in jobs:
            job.future.result()
            self.snapshot(instance, operation, job)
        with self.lock:
            for key, job in jobs:
                if job.acknowledged:
                    self.jobs.pop(key, None)
        return {"instance": instance, "status": "closed"}

    def in_use(self, key):
        with self.lock:
            return any(job.key == key and (not job.future.done() or (job.thread is not None and job.thread.is_alive()))
                       for job in self.jobs.values())


secondary_jobs = SecondaryJobs()
