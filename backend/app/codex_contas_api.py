"""Rotas autenticadas para contas Codex e login nativo."""

from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from pydantic import BaseModel, ConfigDict

from app import codex_contas as accounts
from app.auth import require_auth
from app.mensagens import erro


codex_contas_router = APIRouter(prefix="/api/codex-contas")


class CreateAccountBody(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str


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


@codex_contas_router.post("/{account_id}/prepare", status_code=202,
                          dependencies=[Depends(require_auth)])
async def prepare_codex_account(account_id: str, request: Request) -> dict:
    account = _account(account_id)
    try:
        return await _service(request).prepare(account)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None


@codex_contas_router.get("/{account_id}/prepare", dependencies=[Depends(require_auth)])
def codex_account_preparation(account_id: str, request: Request) -> dict:
    account = _account(account_id)
    return _service(request).preparation_status(account)


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
    return _service(request).login_status(account)


@codex_contas_router.delete("/{account_id}/login", dependencies=[Depends(require_auth)])
async def cancel_codex_login(account_id: str, request: Request,
                             attempt_id: str = Query(min_length=1, max_length=128)) -> dict:
    account = _account(account_id)
    try:
        return await _service(request).cancel_login(account, attempt_id)
    except accounts.AccountError as exc:
        raise _account_error(exc) from None
