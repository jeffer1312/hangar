"""Login nativo e coordenação de operações por conta Codex."""

from __future__ import annotations

import asyncio
import copy
import hashlib
import logging
from dataclasses import dataclass
from pathlib import Path
import threading
import time
import tomllib
import uuid
from collections.abc import Awaitable, Callable

from app import codex_contas as accounts
from app import codex_contas_sync, diag
from app.codex_importador import CodexAusente, CodexNativo, CodexNativoErro


_log = logging.getLogger("hangar.codex.contas")

_LOGIN_COMPLETED = "account/login/completed"
_LOGIN_TIMEOUT = 15 * 60.0
_REQUEST_TIMEOUT = 30.0
_AUTH_TTL = 60.0
# O Codex avisa `success` ANTES de a credencial ficar legível pelo app-server que conduziu o
# login: medido, o auth.json foi gravado no mesmo segundo em que `account/read` ainda devolvia
# conta nenhuma, e o login bem-sucedido virava "falhou" na tela.
_AUTH_APOS_LOGIN = 10.0
_AUTH_APOS_LOGIN_INTERVALO = 0.5
# Quanto tempo um "indisponivel" (app-server que nao respondeu) vale sem tentar de novo.
_INDISPONIVEL_TTL = 20.0


def _error(code: str, **params) -> dict:
    return {"code": code, "params": {k: str(v) for k, v in params.items()}}


def _default_account_in_use(account: accounts.Account) -> bool:
    from app.adapters.codex import sessions

    for meta in sessions.list_all():
        rollout = meta.get("rollout_path") if isinstance(meta, dict) else None
        if not rollout:
            continue
        if not codex_session_alive(meta.get("name"), meta):
            continue
        owner = accounts.account_for_rollout(Path(rollout))
        if owner is not None and owner.id == account.id:
            return True
    return False


def codex_session_alive(name: str | None, meta: dict | None) -> bool:
    """Prova vida pelo app-server ou pelo provider do pane, nunca pelo nome do tmux."""
    from app import procinfo, registry, tmux

    if isinstance(meta, dict) and isinstance(meta.get("app_pid"), int):
        if procinfo.pid_vivo(meta["app_pid"]):
            return True
    if not isinstance(name, str) or not name:
        return False
    panes = tmux.list_panes_of(name)
    children = procinfo._proc_children_map()
    return any(registry.agente_do_pane(pane.get("pid"), children)[0] == "codex"
               for pane in panes if pane.get("pid"))


@dataclass
class _Reservation:
    service: "CodexContasLogin"
    key: str
    token: str
    kind: str
    live: bool = False
    identity: dict | None = None

    def mark_live(self, session_name: str, *, pane_id: str | None = None,
                  pid: int | None = None) -> None:
        self.service._mark_live(self, session_name, pane_id=pane_id, pid=pid)

    def release(self) -> None:
        self.service._release_reservation(self)


@dataclass
class _Attempt:
    account: accounts.Account
    attempt_id: str
    status: str = "starting"
    login_id: str | None = None
    verification_url: str | None = None
    user_code: str | None = None
    auth: dict | None = None
    error: dict | None = None
    native: object | None = None
    queue: asyncio.Queue | None = None
    task: asyncio.Task | None = None
    ready: asyncio.Future | None = None
    cancelled: bool = False
    reservation: _Reservation | None = None


class CodexContasLogin:
    """Dono das tentativas efêmeras e da reserva compartilhada por CODEX_HOME."""

    def __init__(self, *, native=CodexNativo,
                 account_in_use: Callable[[accounts.Account], bool] | None = None,
                 atualizar_principal: Callable[[bool], Awaitable[dict | None]] | None = None):
        self.native = native
        self.account_in_use = account_in_use or _default_account_in_use
        self.atualizar_principal = atualizar_principal
        self._attempts: dict[str, _Attempt] = {}
        self._reservations: dict[str, list[_Reservation]] = {}
        self._preparations: dict[str, asyncio.Task] = {}
        self._preparation_force: set[str] = set()
        self._preparing_source: set[str] = set()
        self._preparation_results: dict[str, dict] = {}
        self._auth_cache: dict[str, tuple[tuple, int, float, dict]] = {}
        self._auth_generation: dict[str, int] = {}
        self._indisponivel: dict[str, tuple[tuple, float]] = {}
        self._lock = threading.RLock()

    @staticmethod
    def _key(account: accounts.Account) -> str:
        return str(account.home.expanduser().resolve(strict=False))

    def _live(self, account: accounts.Account) -> bool:
        try:
            return bool(self.account_in_use(account))
        except Exception as exc:  # noqa: BLE001 - falha de inspeção deve bloquear troca de identidade
            raise accounts.AccountError(409, "codex_account_usage_unknown") from exc

    def _reserve(self, account: accounts.Account, kind: str) -> _Reservation:
        key = self._key(account)
        with self._lock:
            current = self._reservations.get(key, [])
            if kind == "login" and (self._live(account) or any(item.live for item in current)):
                raise accounts.AccountError(409, "codex_account_in_use", {"account_id": account.id})
            active = next((item for item in current if not item.live), None)
            if active is not None:
                code = {
                    "login": "codex_account_login_in_progress",
                    "prepare": "codex_account_preparing",
                }.get(active.kind, "codex_account_creation_in_progress")
                raise accounts.AccountError(409, code, {"account_id": account.id})
            reservation = _Reservation(self, key, uuid.uuid4().hex, kind)
            self._reservations.setdefault(key, []).append(reservation)
            return reservation

    def reserve_creation(self, account: accounts.Account) -> _Reservation:
        """Reserva síncrona para a Task5 manter até o ``to_thread`` terminar."""
        return self._reserve(account, "creation")

    def _mark_live(self, reservation: _Reservation, session_name: str, *,
                   pane_id: str | None = None, pid: int | None = None) -> None:
        with self._lock:
            if reservation in self._reservations.get(reservation.key, []):
                reservation.live = True
                reservation.kind = "live"
                reservation.identity = {"name": session_name, "pane_id": pane_id, "pid": pid}

    def _release_reservation(self, reservation: _Reservation) -> None:
        with self._lock:
            reservations = self._reservations.get(reservation.key, [])
            if reservation in reservations:
                reservations.remove(reservation)
                if not reservations:
                    self._reservations.pop(reservation.key, None)

    def _auth_signature(self, account: accounts.Account) -> tuple:
        values = []
        for name in ("auth.json", "config.toml"):
            path = account.home / name
            try:
                raw = path.read_bytes()
                values.append((name, hashlib.sha256(raw).hexdigest()))
            except OSError:
                values.append((name, None))
        return tuple(values)

    def _auth_public(self, result: dict) -> dict:
        if not isinstance(result, dict) or "account" not in result:
            return {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
        account = result["account"]
        if account is None:
            return {"method": "none", "status": "disconnected", "email": None, "plan": None}
        if not isinstance(account, dict):
            return {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
        kind = account.get("type")
        if kind == "chatgpt":
            return {"method": "oauth", "status": "connected", "email": account.get("email"),
                    "plan": account.get("planType")}
        if kind == "apiKey":
            return {"method": "api_key", "status": "connected", "email": None, "plan": None}
        return {"method": "unknown", "status": "unavailable", "email": None, "plan": None}

    async def _read_auth_native(self, native) -> dict:
        return self._auth_public(await native.request(
            "account/read", {"refreshToken": False}, timeout=_REQUEST_TIMEOUT))

    async def _conta_apos_login(self, native, account: accounts.Account) -> dict:
        """Espera a conta aparecer para o app-server do login; esgotado o prazo, pergunta a um
        app-server novo — se o antigo lembrar de quando não havia credencial, ele nunca veria."""
        limite = time.monotonic() + _AUTH_APOS_LOGIN
        while True:
            auth = await self._read_auth_native(native)
            if auth.get("method") not in ("none", "unknown"):
                return auth
            if time.monotonic() >= limite:
                break
            await asyncio.sleep(_AUTH_APOS_LOGIN_INTERVALO)
        try:
            return await self.read_auth(account, refresh=True)
        except (CodexNativoErro, OSError, RuntimeError, ValueError):
            return auth

    def _invalidate_auth(self, key: str) -> None:
        self._auth_generation[key] = self._auth_generation.get(key, 0) + 1
        self._auth_cache.pop(key, None)
        try:
            from app import codex_models
            codex_models.invalidar(key)
        except (ImportError, OSError):
            pass

    def _cached_auth(self, account: accounts.Account) -> dict:
        key = self._key(account)
        cached = self._auth_cache.get(key)
        if cached and cached[0] == self._auth_signature(account) \
                and cached[1] == self._auth_generation.get(key, 0) \
                and time.monotonic() - cached[2] <= _AUTH_TTL:
            return copy.deepcopy(cached[3])
        return {"method": "unknown", "status": "unavailable", "email": None, "plan": None}

    def cached_auth(self, account: accounts.Account) -> dict | None:
        """Retorna somente a identidade pública já lida pelo serviço, sem iniciar o Codex."""
        key = self._key(account)
        cached = self._auth_cache.get(key)
        if cached and cached[0] == self._auth_signature(account) \
                and cached[1] == self._auth_generation.get(key, 0) \
                and time.monotonic() - cached[2] <= _AUTH_TTL:
            return copy.deepcopy(cached[3])
        return None

    async def read_auth(self, account: accounts.Account, *, refresh: bool = False) -> dict:
        key = self._key(account)
        preparation = self._preparations.get(key)
        if preparation is not None and not preparation.done():
            await asyncio.shield(preparation)
        signature = self._auth_signature(account)
        generation = self._auth_generation.get(key, 0)
        cached = self._auth_cache.get(key)
        if (not refresh and cached and cached[0] == signature and cached[1] == generation
                and time.monotonic() - cached[2] <= _AUTH_TTL):
            return copy.deepcopy(cached[3])
        indisponivel = self._indisponivel.get(key)
        if not refresh and indisponivel and indisponivel[0] == signature \
                and time.monotonic() - indisponivel[1] <= _INDISPONIVEL_TTL:
            return {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
        codigo = "sem_resposta_valida"
        try:
            async with self.native(Path.home(), account.home, account=account) as native:
                result = await self._read_auth_native(native)
        except CodexAusente:
            # Sem Codex instalado nao ha o que consultar: e estado da maquina, nao falha.
            codigo = "cli_ausente"
            result = {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
        except (CodexNativoErro, OSError, RuntimeError, ValueError) as exc:
            diag.registrar("conta.auth.falhou", "erro", provider="codex", etapa="consultar_auth",
                           conta_id=diag.conta_id(key), **diag.erro_campos(exc))
            result = {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
        current_signature = self._auth_signature(account)
        if result["status"] == "unavailable":
            diag.registrar("conta.auth.indisponivel", "aviso", provider="codex", etapa="consultar_auth",
                           conta_id=diag.conta_id(key), codigo=codigo)
            # Tambem se lembra do fracasso: sem isto cada listagem de credenciais subia um
            # app-server por conta e esperava o teto de 3s de cada um (medido: 6,4s no Windows).
            self._indisponivel[key] = (current_signature, time.monotonic())
        elif self._auth_generation.get(key, 0) == generation and current_signature == signature:
            self._auth_cache[key] = (signature, generation, time.monotonic(), copy.deepcopy(result))
        return result

    def _prepared(self, account: accounts.Account) -> None:
        if account.is_default:
            return
        try:
            config = tomllib.loads((account.home / "config.toml").read_text(encoding="utf-8"))
        except (OSError, UnicodeError, tomllib.TOMLDecodeError, ValueError) as exc:
            raise accounts.AccountError(409, "codex_account_prepare_required", {"account_id": account.id}) from exc
        if config.get("cli_auth_credentials_store") != "file":
            raise accounts.AccountError(409, "codex_account_auth_storage_invalid", {"account_id": account.id})

    async def _run_login(self, attempt: _Attempt) -> None:
        native = None
        reservation = attempt.reservation
        deadline = time.monotonic() + _LOGIN_TIMEOUT
        inicio = time.monotonic()
        campos = {"provider": "codex", "operacao": attempt.attempt_id,
                  "conta_id": diag.conta_id(self._key(attempt.account))}
        etapa = "abrir_app_server"
        diag.registrar("conta.login.iniciou", etapa=etapa, **campos)
        try:
            native = self.native(Path.home(), attempt.account.home, account=attempt.account,
                                 timeout=_REQUEST_TIMEOUT)
            attempt.native = native
            await native.__aenter__()
            queue = native.subscribe(_LOGIN_COMPLETED)
            attempt.queue = queue
            etapa = "iniciar_login"
            response = await native.request("account/login/start", {"type": "chatgptDeviceCode"},
                                            timeout=_REQUEST_TIMEOUT)
            login_id = response.get("loginId")
            if not isinstance(login_id, str) or not login_id:
                raise CodexNativoErro("O Codex não retornou o identificador do login.")
            verification_url = response.get("verificationUrl")
            user_code = response.get("userCode")
            if not isinstance(verification_url, str) or not verification_url \
                    or not isinstance(user_code, str) or not user_code:
                raise CodexNativoErro("O Codex retornou dados de login inválidos.")
            attempt.login_id = login_id
            attempt.verification_url = verification_url
            attempt.user_code = user_code
            attempt.status = "waiting"
            etapa = "aguardar_autorizacao"
            diag.registrar("conta.login.aguardando", etapa=etapa, **campos)
            if attempt.ready is not None and not attempt.ready.done():
                attempt.ready.set_result(None)
            if attempt.cancelled:
                await self._cancel_native(attempt)
                return
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError
                event = await asyncio.wait_for(queue.get(), remaining)
                if event is None:
                    raise CodexNativoErro("O Codex encerrou a conexão do login.")
                params = event.get("params") if isinstance(event, dict) else None
                if not isinstance(params, dict) or params.get("loginId") not in (None, login_id):
                    continue
                if not params.get("success"):
                    raise CodexNativoErro("O Codex recusou o login.")
                etapa = "confirmar_credencial"
                diag.registrar("conta.login.confirmando", etapa=etapa, **campos)
                auth = await self._conta_apos_login(native, attempt.account)
                if auth.get("method") in ("none", "unknown"):
                    raise CodexNativoErro("O Codex não confirmou a conta após o login.")
                attempt.auth = auth
                attempt.status = "completed"
                self._invalidate_auth(self._key(attempt.account))
                return
        except asyncio.CancelledError:
            attempt.cancelled = True
            attempt.status = "cancelled"
            raise
        # Cancelar fecha o app-server, e o erro que sobe daí é consequência do pedido, não falha:
        # marcado como erro, a tela mostrava "Login cancelado" e "O login falhou" na mesma frase.
        except TimeoutError:
            attempt.status = "cancelled" if attempt.cancelled else "failed"
            if not attempt.cancelled:
                attempt.error = _error("codex_account_login_timeout")
        except (CodexNativoErro, OSError, RuntimeError, ValueError) as exc:
            attempt.status = "cancelled" if attempt.cancelled else "failed"
            if not attempt.cancelled:
                attempt.error = _error("codex_account_login_failed")
                diag.registrar("conta.login.falhou", "erro", etapa=etapa,
                               **campos, **diag.erro_campos(exc))
                # A tela só pode dizer "falhou"; sem esta linha a causa não fica em lugar nenhum.
                _log.warning("login Codex falhou: conta=%s %s: %s",
                             attempt.account.id, type(exc).__name__, exc)
        finally:
            diag.registrar("conta.login.terminou",
                           "erro" if attempt.status == "failed" else "aviso" if attempt.cancelled else "ok",
                           etapa=etapa, codigo=(attempt.error or {}).get("code", attempt.status),
                           ms=int((time.monotonic() - inicio) * 1000), **campos)
            if attempt.ready is not None and not attempt.ready.done():
                attempt.ready.set_result(None)
            attempt.native = None
            if reservation is not None and not reservation.live:
                self._release_reservation(reservation)
            try:
                if attempt.queue is not None and native is not None:
                    native.unsubscribe(_LOGIN_COMPLETED, attempt.queue)
            finally:
                if native is not None:
                    try:
                        await native.close()
                    except Exception as exc:  # noqa: BLE001 - cleanup não pode prender a tentativa
                        diag.registrar("conta.login.limpeza_falhou", "aviso", etapa="fechar_app_server",
                                       **campos, **diag.erro_campos(exc))

    async def start_login(self, account: accounts.Account) -> dict:
        key = self._key(account)
        with self._lock:
            current = self._attempts.get(key)
            if current is not None and current.status in {"starting", "waiting"}:
                attempt = current
            else:
                self._prepared(account)
                reservation = self._reserve(account, "login")
                attempt = _Attempt(account, uuid.uuid4().hex)
                attempt.reservation = reservation
                attempt.ready = asyncio.get_running_loop().create_future()
                self._attempts[key] = attempt
                attempt.task = asyncio.create_task(self._run_login(attempt))
        await asyncio.shield(attempt.ready)
        return self._public_attempt(attempt)

    def _public_attempt(self, attempt: _Attempt) -> dict:
        result = {
            "account_id": attempt.account.id,
            "attempt_id": attempt.attempt_id,
            "status": "waiting" if attempt.status == "starting" else attempt.status,
        }
        if attempt.error is not None:
            result["error"] = copy.deepcopy(attempt.error)
        if attempt.verification_url is not None:
            result["verification_url"] = attempt.verification_url
        if attempt.user_code is not None:
            result["user_code"] = attempt.user_code
        return result

    def login_status(self, account: accounts.Account) -> dict | None:
        attempt = self._attempts.get(self._key(account))
        return self._public_attempt(attempt) if attempt is not None else None

    async def _cancel_native(self, attempt: _Attempt) -> None:
        attempt.cancelled = True
        attempt.status = "cancelled"
        campos = {"provider": "codex", "operacao": attempt.attempt_id,
                  "conta_id": diag.conta_id(self._key(attempt.account))}
        diag.registrar("conta.login.cancelou", "aviso", etapa="cancelar", **campos)
        self._invalidate_auth(self._key(attempt.account))
        native = attempt.native
        if native is None:
            return
        try:
            if attempt.login_id:
                try:
                    await native.request("account/login/cancel", {"loginId": attempt.login_id},
                                         timeout=_REQUEST_TIMEOUT)
                except (CodexNativoErro, OSError, RuntimeError) as exc:
                    diag.registrar("conta.login.cancelamento_falhou", "aviso", etapa="cancelar_nativo",
                                   **campos, **diag.erro_campos(exc))
        finally:
            try:
                await native.close()
            except Exception as exc:  # noqa: BLE001 - cancelamento deve sempre liberar a reserva
                diag.registrar("conta.login.limpeza_falhou", "aviso", etapa="fechar_app_server",
                               **campos, **diag.erro_campos(exc))

    async def cancel_login(self, account: accounts.Account, attempt_id: str) -> dict:
        attempt = self._attempts.get(self._key(account))
        if attempt is None or attempt.attempt_id != attempt_id:
            raise accounts.AccountError(409, "codex_login_attempt_mismatch", {"account_id": account.id})
        if attempt.status in {"completed", "failed", "cancelled"}:
            return self._public_attempt(attempt)
        attempt.cancelled = True
        await self._cancel_native(attempt)
        if attempt.task is not None:
            await asyncio.shield(attempt.task)
        return self._public_attempt(attempt)

    async def prepare(self, account: accounts.Account, *, forcar: bool = False) -> dict:
        if account.is_default:
            return {"status": "ready", "trust_pending": False, "issues": []}
        key = self._key(account)
        with self._lock:
            task = self._preparations.get(key)
            if task is not None and not task.done():
                if forcar:
                    self._preparation_force.add(key)
            else:
                reservation = self._reserve(account, "prepare")
                if self.atualizar_principal is not None:
                    self._preparing_source.add(key)
                task = asyncio.create_task(self._prepare_one(account, reservation, forcar))
                self._preparations[key] = task
                self._preparation_results.pop(key, None)
        return self.preparation_status(account) if not task.done() else task.result()

    @diag.rastrear("conta.preparar", provider="codex")
    async def _prepare_one(self, account: accounts.Account, reservation: _Reservation,
                           forcar: bool) -> dict:
        key = self._key(account)
        etapa = "principal"
        diag.registrar("conta.preparar.etapa", provider="codex", etapa=etapa, conta_id=diag.conta_id(key))
        try:
            while True:
                principal = None
                if self.atualizar_principal is not None:
                    etapa = "principal"
                    self._preparing_source.add(key)
                    try:
                        principal = await self.atualizar_principal(forcar)
                    finally:
                        self._preparing_source.discard(key)
                etapa = "herdar_configuracao"
                diag.registrar("conta.preparar.etapa", provider="codex", etapa=etapa)
                if forcar:
                    result = await codex_contas_sync.prepare_account(account, force=True)
                else:
                    result = await codex_contas_sync.prepare_account(account)
                if isinstance(principal, dict) and principal.get("estado") not in (None, "ok", "ocioso"):
                    result = copy.deepcopy(result)
                    if result.get("status") == "ready":
                        result["status"] = "partial"
                    result.setdefault("issues", []).insert(0, {
                        "code": "codex_account_source_sync_incomplete",
                        "params": {"status": str(principal.get("estado"))},
                    })
                with self._lock:
                    repetir = key in self._preparation_force and not forcar
                    self._preparation_force.discard(key)
                if not repetir:
                    self._preparation_results[key] = copy.deepcopy(result)
                    status = result.get("status", "unknown")
                    issues = result.get("issues") or []
                    diag.registrar("conta.preparar.concluiu",
                                   "ok" if status == "ready" else "erro" if status == "error" else "aviso",
                                   provider="codex", etapa=etapa, codigo=status, quantidade=len(issues))
                    for issue in issues:
                        diag.registrar("conta.preparar.aviso", "aviso", provider="codex", etapa=etapa,
                                       codigo=issue.get("code", "unknown"))
                    return result
                forcar = True
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # noqa: BLE001 - estado público não pode expor caminho/segredo
            diag.registrar("conta.preparar.falhou", "erro", provider="codex", etapa=etapa,
                           **diag.erro_campos(exc))
            result = {"status": "error", "trust_pending": False,
                      "issues": [{"code": "codex_account_prepare_failed",
                                  "params": {"error": type(exc).__name__}}]}
            self._preparation_results[key] = copy.deepcopy(result)
            return result
        finally:
            self._preparing_source.discard(key)
            reservation.release()

    def preparation_status(self, account: accounts.Account) -> dict:
        task = self._preparations.get(self._key(account))
        gravado = codex_contas_sync.preparation_status(account)
        if task is not None and not task.done():
            # A task viva decide o status; a etapa vem do disco, que é ela mesma quem atualiza.
            return {"status": "running", "trust_pending": False, "issues": [],
                    "etapa": "principal" if self._key(account) in self._preparing_source else gravado.get("etapa")}
        return copy.deepcopy(self._preparation_results.get(self._key(account), gravado))

    @diag.rastrear("conta.apagar", provider="codex")
    async def delete_account(self, account: accounts.Account) -> None:
        # Mesma trava do login: conta com sessao viva, login ou preparo em andamento nao sai.
        reservation = self._reserve(account, "login")
        try:
            diag.registrar("conta.apagar.etapa", provider="codex", etapa="remover_pasta",
                           conta_id=diag.conta_id(self._key(account)))
            await asyncio.to_thread(accounts.delete_account, account)
            key = self._key(account)
            with self._lock:
                self._auth_cache.pop(key, None)
                self._preparations.pop(key, None)
                self._preparation_force.discard(key)
                self._preparation_results.pop(key, None)
        finally:
            reservation.release()

    @diag.rastrear("conta.criar", provider="codex")
    async def create_account(self, name: str) -> dict:
        # Só a pasta: herdar a padrão (plugins, hooks, MCPs) leva minutos e é oferecido depois do login.
        diag.registrar("conta.criar.etapa", provider="codex", etapa="criar_pasta")
        account = await asyncio.to_thread(accounts.create_account, name)
        diag.registrar("conta.criar.concluiu", provider="codex", conta_id=diag.conta_id(self._key(account)))
        return await self.account_snapshot(account, read_auth=False)

    @staticmethod
    def _has_settings(account: accounts.Account) -> bool:
        """A padrão tem algo que valha herdar? Recém-logada e nunca aberta, não tem."""
        if not account.is_default:
            return False
        home = account.home
        try:
            if (home / "config.toml").is_file():
                return True
            return any((home / d).is_dir() and any((home / d).iterdir())
                       for d in ("agents", "skills", "hooks", "plugins"))
        except OSError:
            return False

    async def account_snapshot(self, account: accounts.Account, *, read_auth: bool = True,
                               sync: dict | None = None) -> dict:
        sync = sync if sync is not None else self.preparation_status(account)
        if read_auth:
            auth = await self.read_auth(account)
        elif sync.get("status") == "running":
            auth = self._cached_auth(account)
        else:
            auth = {"method": "none", "status": "disconnected", "email": None, "plan": None}
        home = str(account.home.expanduser().resolve(strict=False))
        return {"id": account.id, "credential_id": f"codex:{home}", "name": account.id,
                "home": home, "is_default": account.is_default, "auth": auth,
                "sync": self.preparation_status(account),
                "has_settings": self._has_settings(account)}

    async def accounts_snapshot(self) -> list[dict]:
        result = []
        for account in accounts.list_accounts():
            sync = self.preparation_status(account)
            result.append(await self.account_snapshot(
                account, read_auth=sync.get("status") != "running", sync=sync))
        return result

    async def close(self) -> None:
        attempts = list(self._attempts.values())
        for attempt in attempts:
            if attempt.status in {"starting", "waiting"}:
                attempt.cancelled = True
                await self._cancel_native(attempt)
        tasks = [attempt.task for attempt in attempts if attempt.task is not None]
        if tasks:
            await asyncio.gather(*(asyncio.shield(task) for task in tasks), return_exceptions=True)
        prep = list(self._preparations.values())
        if prep:
            await asyncio.gather(*(asyncio.shield(task) for task in prep), return_exceptions=True)
