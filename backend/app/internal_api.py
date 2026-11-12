# backend/app/internal_api.py
"""Rotas internas que só o hangar-server, filho deste backend na mesma máquina, consome."""
import asyncio
import secrets
import json
import logging

from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, ConfigDict

from app import diag
from app.internal_pool import internal_thread
from app.mensagens import erro

_LOOPBACK = {"127.0.0.1", "::1"}

# Só na memória: no os.environ ele iria para toda sessão que o backend sobe.
_secret: str | None = None


def set_secret(value: str | None) -> None:
    global _secret
    _secret = value or None


async def require_internal(request: Request) -> None:
    # No laço, sem thread: um pool cheio não pode impedir o Rust de entrar nas rotas que liberam o pool.
    # 404 em toda recusa: quem não é o hangar-server não descobre que a rota existe. O segredo nasce
    # a cada subida e o repasse do hangar-server manda o IP real no X-Forwarded-For, então quem
    # chega de fora pela porta pública cai no IP antes do segredo.
    secret = _secret
    ip = request.client.host if request.client else None
    given = request.headers.get("x-hangar-internal", "")
    if secret is None or ip not in _LOOPBACK or not secrets.compare_digest(given.encode(), secret.encode()):
        # Pedido local COM o cabeçalho é o hangar-server: a recusa desliga o atalho dele sem
        # erro visível, então vai ao diário (nunca o valor do segredo).
        if ip in _LOOPBACK and "x-hangar-internal" in request.headers:
            diag.registrar("internal.recusado", "aviso",
                           codigo="sem_segredo" if secret is None else "segredo_errado")
        raise HTTPException(status_code=404)


router = APIRouter(prefix="/internal", dependencies=[Depends(require_internal)], include_in_schema=False)


@router.get("/ping")
async def ping() -> dict:
    """Sem resposta, o laço do Python está parado (vigia do Rust)."""
    return {"ok": True}


async def _instance_body(request: Request, limit: int, *, modes=None):
    """Só a instância viva do coordenador fala por aqui; o corpo tem teto antes do JSON."""
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    instance = request.headers.get("x-hangar-runtime-instance", "")
    if (coordinator is None or not coordinator.instance
            or (modes is not None and getattr(coordinator, "mode", "python") not in modes)
            or not secrets.compare_digest(instance, coordinator.instance)):
        raise HTTPException(404)
    raw = bytearray()
    async for chunk in request.stream():
        raw.extend(chunk)
        if len(raw) > limit:
            raise HTTPException(413)
    return coordinator, instance, raw


@router.post("/accounts/prepare")
@router.post("/accounts/prepare/wait")
async def account_prepare(request: Request):
    from app import account_bridge
    _, instance, raw = await _instance_body(request, 16 * 1024)
    try:
        body = json.loads(raw)
        if request.url.path.endswith("/wait"):
            if not isinstance(body, dict) or set(body) != {"operation"} or not isinstance(body["operation"], str):
                raise ValueError("operação inválida")
            return await account_bridge.preparation_jobs.wait(body["operation"])
        if not isinstance(body, dict) or body.get("instance") != instance:
            raise ValueError("instância divergente")
        return await account_bridge.preparation_jobs.start(body)
    except (KeyError, TypeError, ValueError, OSError):
        raise HTTPException(409, detail={"code": "account_prepare_rejected"}) from None


@router.get("/migration/status")
async def migration_status() -> dict:
    from app import migration_status
    return await internal_thread(migration_status.facts)
