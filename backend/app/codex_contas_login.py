"""Ponte de autenticação Codex para o Rust.

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

from app import codex_contas as accounts
from app import account_lifecycle
from app.account_bridge import NEED_RUST, request_codex, request_preparation
from app.codex_contas_sync import _issue


_log = logging.getLogger("hangar.codex.contas")

# Autenticação Codex que não deu pra ler; sem o Rust, com o motivo.
UNAVAILABLE_AUTH = {"method": "unknown", "status": "unavailable", "email": None, "plan": None}
_NEED_RUST_AUTH = {**UNAVAILABLE_AUTH, "reason": NEED_RUST["code"]}
_NEED_RUST_PREPARATION = {"status": "error", "trust_pending": False, "issues": [_issue(NEED_RUST["code"])]}


class CodexContasLogin:
    """Leitura de autenticação e preparo das contas Codex, pela ponte do Rust."""

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
