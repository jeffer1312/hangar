"""Reserva por conta Codex durante a criação de sessão e ponte de autenticação para o Rust.

Login, logout, preparo e leitura de autenticação são do Rust; sem ele não há reserva Python e a
autenticação fica indisponível, sem abrir o Codex aqui.
"""

from __future__ import annotations

import asyncio
import copy
import logging
from dataclasses import dataclass
from pathlib import Path
import threading
import time
import uuid
from collections.abc import Awaitable, Callable

from app import codex_contas as accounts
from app import account_lifecycle
from app.account_bridge import NEED_RUST, request_codex, request_preparation
from app.codex_contas_sync import _issue


_log = logging.getLogger("hangar.codex.contas")

# Autenticação Codex que não deu pra ler; sem o Rust, com o motivo.
UNAVAILABLE_AUTH = {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
_NEED_RUST_AUTH = {**UNAVAILABLE_AUTH, "reason": NEED_RUST["code"]}
_NEED_RUST_PREPARATION = {"status": "error", "trust_pending": False, "issues": [_issue(NEED_RUST["code"])]}


@dataclass
class _Reservation:
    service: "CodexContasLogin"
    key: str
    token: str
    live: bool = False
    guard: account_lifecycle.AccountGuard | None = None
    birth_path: Path | None = None

    def mark_live(self, session_name: str) -> None:
        self.service._mark_live(self, session_name)

    def retire_birth(self) -> None:
        if self.birth_path is not None and account_lifecycle.retire_terminal_birth(self.birth_path):
            self.birth_path = None

    def release(self) -> None:
        self.service._release_reservation(self)


class CodexContasLogin:
    """Dono da reserva compartilhada por CODEX_HOME enquanto uma sessão nasce."""

    def __init__(self, *, atualizar_principal: Callable[[bool], Awaitable[dict | None]] | None = None):
        self.atualizar_principal = atualizar_principal
        self._reservations: dict[str, list[_Reservation]] = {}
        self._lock = threading.RLock()

    @staticmethod
    def _key(account: accounts.Account) -> str:
        return str(account.home.expanduser().resolve(strict=False))

    def reserve_creation(self, account: accounts.Account) -> _Reservation:
        """Reserva síncrona para a criação manter até o ``to_thread`` terminar."""
        key = self._key(account)
        with self._lock:
            if any(not item.live for item in self._reservations.get(key, [])):
                raise accounts.AccountError(409, "codex_account_creation_in_progress",
                                            {"account_id": account.id})
            reservation = _Reservation(self, key, uuid.uuid4().hex)
            try:
                reservation.guard = account_lifecycle.acquire(
                    account_lifecycle.AccountKey.new("codex", account.home),
                    account_lifecycle.GuardMode.SHARED, deadline=time.monotonic())
            except account_lifecycle.AccountLockError as exc:
                raise accounts.AccountError(409, "codex_account_in_use", {"account_id": account.id}) from exc
            self._reservations.setdefault(key, []).append(reservation)
            return reservation

    def _mark_live(self, reservation: _Reservation, session_name: str) -> None:
        with self._lock:
            if reservation not in self._reservations.get(reservation.key, []):
                return
            guard = reservation.guard.retain()
        # O worker possui o descritor, mas consulta e disco não retêm o lock do serviço.
        with guard:
            birth_path = account_lifecycle.publish_terminal_birth(guard, session_name, reservation.token)
            with self._lock:
                reservation.birth_path = birth_path
                if reservation not in self._reservations.get(reservation.key, []):
                    return
                reservation.live = True
                original = reservation.guard
                reservation.guard = None
            if original is not None:
                original.close()

    def _release_reservation(self, reservation: _Reservation) -> None:
        with self._lock:
            if reservation.guard is not None:
                reservation.guard.close()
                reservation.guard = None
            reservations = self._reservations.get(reservation.key, [])
            if reservation in reservations:
                reservations.remove(reservation)
                if not reservations:
                    self._reservations.pop(reservation.key, None)

    def _invalidate_auth(self, key: str) -> None:
        """O login da conta mudou no Rust: o catálogo de modelos dela vale de novo."""
        try:
            from app import codex_models
            codex_models.invalidar(key)
        except (ImportError, OSError):
            pass

    async def aquecer(self) -> None:
        """Pede ao Rust o login das contas visíveis uma vez, na subida: a primeira abertura da
        criação de sessão encontra o cache dele pronto. Falha fica pra leitura normal."""
        for account in accounts.list_visible_accounts():
            try:
                await self.read_auth(account)
            except Exception:  # noqa: BLE001 — aquecimento é só adiantamento
                _log.debug("aquecimento do login Codex falhou: %s", account.id, exc_info=True)

    async def read_auth_rapido(self, account: accounts.Account) -> dict:
        """O último login que o Rust conhece, sem esperar uma leitura nova."""
        result = await asyncio.to_thread(request_codex, "auth_cached", account)
        return dict(_NEED_RUST_AUTH) if result is None else result

    async def read_auth(self, account: accounts.Account, *, refresh: bool = False) -> dict:
        result = await asyncio.to_thread(request_codex, "auth", account, refresh=refresh)
        return dict(_NEED_RUST_AUTH) if result is None else result

    def preparation_status(self, account: accounts.Account) -> dict:
        return request_preparation(account) or copy.deepcopy(_NEED_RUST_PREPARATION)

    async def preparation_status_async(self, account: accounts.Account) -> dict:
        """Só a ida HTTP à ponte vai para thread."""
        return await asyncio.to_thread(request_preparation, account) or copy.deepcopy(_NEED_RUST_PREPARATION)
