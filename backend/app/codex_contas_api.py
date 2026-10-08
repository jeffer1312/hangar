"""Rotas autenticadas para contas Codex e login nativo."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import threading
import time
from pathlib import Path
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from pydantic import BaseModel, ConfigDict, Field

from app import atomico, codex_appserver, cotas, log_paths
from app import codex_contas as accounts
from app.auth import require_auth
from app.mensagens import erro


codex_contas_router = APIRouter(prefix="/api/codex-contas")
_log = logging.getLogger("hangar.codex.contas")

_RESET_ATTEMPT_TTL_S = 86400
_reset_attempts: dict[tuple[str, str | None, str], float] = {}
_reset_attempts_lock = threading.Lock()


class CreateAccountBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str


class ConsumeResetBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    credit_id: str | None = Field(default=None, max_length=256)
    idempotency_key: UUID


class _ResetError(RuntimeError):
    def __init__(self, status: int, code: str, message: str, **params):
        super().__init__(message)
        self.status, self.code, self.message, self.params = status, code, message, params


def _arquivo_tentativas() -> Path:
    return log_paths.base().parent / "codex-reset-attempts.json"


def _ler_tentativas() -> dict[tuple[str, str | None, str], float]:
    alvo = _arquivo_tentativas()
    try:
        linhas = alvo.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return dict(_reset_attempts)
    except OSError as exc:
        raise _ResetError(503, "codex_reset_failed",
                          "não foi possível conferir a tentativa anterior") from exc
    tentativas = dict(_reset_attempts)
    ilegiveis = 0
    for linha in linhas:
        try:
            item = json.loads(linha)
            account_id = item["account_id"]
            credit_id = item.get("credit_id")
            key = item["idempotency_key"]
            accepted_at = float(item["accepted_at"])
        except (AttributeError, KeyError, TypeError, ValueError):
            ilegiveis += 1
            continue
        if isinstance(account_id, str) and isinstance(key, str):
            tentativas[(account_id, credit_id if isinstance(credit_id, str) else None, key)] = accepted_at
        else:
            ilegiveis += 1
    if ilegiveis:
        _log.warning("tentativas de redefinicao Codex: %d linha(s) ilegivel(is) em %s", ilegiveis, alvo)
    return tentativas


def _gravar_tentativas(tentativas: dict[tuple[str, str | None, str], float]) -> None:
    # Reescreve só as vigentes: o arquivo não cresce e linha ilegível sai na próxima gravação.
    alvo = _arquivo_tentativas()
    alvo.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    tmp = alvo.with_name(alvo.name + ".tmp")
    with tmp.open("w", encoding="utf-8") as arquivo:
        for (account_id, credit_id, key), accepted_at in tentativas.items():
            arquivo.write(json.dumps({"account_id": account_id, "credit_id": credit_id,
                                      "idempotency_key": key, "accepted_at": accepted_at}) + "\n")
        arquivo.flush()
        os.fsync(arquivo.fileno())
    atomico.substituir(tmp, alvo)


def _tentativas_atuais() -> dict[tuple[str, str | None, str], float]:
    agora = time.time()
    return {tentativa: ts for tentativa, ts in _ler_tentativas().items()
            if agora - _RESET_ATTEMPT_TTL_S <= ts <= agora + 300}


def _tentativa_anterior(account_id: str, credit_id: str | None, key: str) -> bool:
    global _reset_attempts
    with _reset_attempts_lock:
        _reset_attempts = _tentativas_atuais()
        return (account_id, credit_id, key) in _reset_attempts


def _guardar_tentativa(account_id: str, credit_id: str | None, key: str) -> None:
    global _reset_attempts
    with _reset_attempts_lock:
        tentativas = _tentativas_atuais()
        tentativas[(account_id, credit_id, key)] = time.time()
        try:
            _gravar_tentativas(tentativas)
        except OSError as exc:
            raise _ResetError(503, "codex_reset_failed",
                              "não foi possível guardar a tentativa de redefinição") from exc
        _reset_attempts = tentativas


def _perguntar_reset(account: accounts.Account, etapa: str, metodo: str,
                     params: dict | None = None) -> dict:
    try:
        return codex_appserver.perguntar(
            metodo, codex_home=account.home, params=params)
    except (codex_appserver.CodexIndisponivel, codex_appserver.CodexRecusado,
            codex_appserver.CodexRespostaInvalida) as exc:
        _log.error("redefinicao Codex falhou conta=%s etapa=%s causa=%s",
                   account.id, etapa, type(exc).__name__)
        raise


def _consume_reset(account: accounts.Account, body: ConsumeResetBody) -> dict:
    atual = _perguntar_reset(account, "leitura", "account/rateLimits/read")
    semanal = cotas.codex_weekly_used(atual)
    chave = str(body.idempotency_key)
    repeticao = _tentativa_anterior(account.id, body.credit_id, chave)
    if semanal is None:
        raise _ResetError(409, "codex_reset_weekly_unavailable",
                          "não foi possível confirmar a cota semanal")
    if semanal < 100 and not repeticao:
        raise _ResetError(409, "codex_reset_weekly_not_exhausted",
                          "a cota semanal ainda não acabou", pct=round(semanal))
    redefinicoes = cotas.codex_reset_credits(atual)
    if not repeticao and (redefinicoes is None or redefinicoes.available_count < 1):
        return {"outcome": "noCredit"}
    if not repeticao:
        _guardar_tentativa(account.id, body.credit_id, chave)
    params = {"idempotencyKey": chave}
    if body.credit_id:
        params["creditId"] = body.credit_id
    resposta = _perguntar_reset(
        account, "consumo", "account/rateLimitResetCredit/consume", params)
    outcome = resposta.get("outcome")
    if outcome not in ("reset", "nothingToReset", "noCredit", "alreadyRedeemed"):
        raise _ResetError(502, "codex_reset_invalid_response",
                          "o Codex devolveu uma resposta inválida ao redefinir a cota")
    return {"outcome": outcome}


def _service(request: Request):
    try:
        return request.app.state.codex_contas_login
    except AttributeError:
        raise HTTPException(503, detail=erro("codex_account_service_unavailable",
                                             "serviço de contas Codex indisponível")) from None


def _account(account_id: str) -> accounts.Account:
    try:
        return accounts.resolve_account(account_id)
    except accounts.AccountError as exc:
        messages = {
            "codex_account_invalid_name": "nome de conta Codex inválido",
            "codex_account_not_found": "conta Codex não encontrada",
            "codex_account_invalid_marker": "conta Codex inválida",
        }
        raise HTTPException(exc.status, detail=erro(exc.code, messages.get(exc.code, "operação de conta Codex recusada"),
                                                     **exc.params)) from None


def _account_error(exc: accounts.AccountError) -> HTTPException:
    messages = {
        "codex_account_exists": "conta Codex já existe",
        "codex_account_in_use": "conta Codex está em uso",
        "codex_account_login_in_progress": "já existe login em andamento para esta conta",
        "codex_account_creation_in_progress": "já existe criação em andamento para esta conta",
        "codex_account_preparing": "a preparação da conta Codex está em andamento",
        "codex_account_prepare_required": "prepare a conta Codex antes do login",
        "codex_account_auth_storage_invalid": "a conta Codex precisa usar armazenamento em arquivo",
        "codex_login_attempt_mismatch": "a tentativa de login já mudou",
        "codex_account_default_protected": "a conta padrão do Codex não pode ser apagada",
        "codex_account_invalid_marker": "conta Codex inválida",
        "codex_account_delete_failed": "não foi possível apagar a conta Codex",
    }
    return HTTPException(exc.status, detail=erro(exc.code, messages.get(exc.code, "operação de conta Codex recusada"),
                                                  **exc.params))


@codex_contas_router.get("", dependencies=[Depends(require_auth)])
async def list_codex_accounts(request: Request) -> list[dict]:
    return await _service(request).accounts_snapshot()


@codex_contas_router.post("", status_code=201, dependencies=[Depends(require_auth)])
async def create_codex_account(body: CreateAccountBody, request: Request) -> dict:
    try:
        return await _service(request).create_account(body.name)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None


@codex_contas_router.delete("/{account_id}", dependencies=[Depends(require_auth)])
async def delete_codex_account(account_id: str, request: Request) -> dict:
    account = _account(account_id)
    try:
        await _service(request).delete_account(account)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None
    # Corpo JSON, nao 204: o apiFetchForServer do core sempre faz res.json().
    return {"ok": True}


@codex_contas_router.post("/{account_id}/rate-limit-reset", dependencies=[Depends(require_auth)])
async def consume_rate_limit_reset(account_id: str, body: ConsumeResetBody) -> dict:
    account = _account(account_id)
    try:
        return await asyncio.to_thread(_consume_reset, account, body)
    except _ResetError as exc:
        raise HTTPException(exc.status, detail=erro(exc.code, exc.message, **exc.params)) from None
    except (codex_appserver.CodexIndisponivel, codex_appserver.CodexRecusado,
            codex_appserver.CodexRespostaInvalida) as exc:
        raise HTTPException(502, detail=erro("codex_reset_failed",
                                             "não foi possível redefinir a cota do Codex")) from exc


@codex_contas_router.post("/{account_id}/prepare", status_code=202,
                          dependencies=[Depends(require_auth)])
async def prepare_codex_account(account_id: str, request: Request,
                                forcar: bool = Query(False)) -> dict:
    account = _account(account_id)
    try:
        return await _service(request).prepare(account, forcar=forcar)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None


@codex_contas_router.get("/{account_id}/prepare", dependencies=[Depends(require_auth)])
def codex_account_preparation(account_id: str, request: Request,
                              cwd: str | None = Query(None, max_length=4096)) -> dict:
    account = _account(account_id)
    from app.account_bridge import request_preparation
    delegated = request_preparation(account, cwd=cwd)
    if delegated is not None:
        return delegated
    result = _service(request).preparation_status(account)
    if cwd and result.get("status") in ("ready", "partial"):
        from app.adapters.codex import sessions
        sessions.pretrust_cwd(cwd, codex_home=account.home)
    return result


@codex_contas_router.post("/{account_id}/login", dependencies=[Depends(require_auth)])
async def start_codex_login(account_id: str, request: Request) -> dict:
    account = _account(account_id)
    try:
        return await _service(request).start_login(account)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None


@codex_contas_router.get("/{account_id}/login", dependencies=[Depends(require_auth)])
def codex_login_status(account_id: str, request: Request) -> dict | None:
    account = _account(account_id)
    try:
        return _service(request).login_status(account)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None


@codex_contas_router.delete("/{account_id}/login", dependencies=[Depends(require_auth)])
async def cancel_codex_login(account_id: str, request: Request,
                             attempt_id: str = Query(min_length=1, max_length=128)) -> dict:
    account = _account(account_id)
    try:
        return await _service(request).cancel_login(account, attempt_id)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None
