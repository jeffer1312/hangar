"""Rotas do painel de saúde dos harnesses (app/harness_saude.py)."""
import asyncio
import sqlite3
import subprocess

from fastapi import APIRouter, Depends, HTTPException

from app import contas, harness_saude
from app.auth import require_auth
from app.mensagens import erro

harness_router = APIRouter(prefix="/api/harness")


@harness_router.get("", dependencies=[Depends(require_auth)])
async def listar() -> list[dict]:
    # `--version` de cinco CLIs em série: fora do loop do servidor, que segue servindo o SSE.
    return await asyncio.to_thread(harness_saude.diagnosticar)


@harness_router.post("/conserto/{id_:path}", dependencies=[Depends(require_auth)])
async def consertar(id_: str) -> dict:
    try:
        feito = await asyncio.to_thread(harness_saude.consertar, id_)
    except ValueError as e:
        raise HTTPException(400, detail=erro("erro_harness_conserto", str(e), motivo=str(e)))
    except (OSError, contas.ContaError, sqlite3.Error, subprocess.SubprocessError) as e:
        raise HTTPException(500, detail=erro("erro_harness_conserto", str(e), motivo=str(e)))
    return {"feito": feito, "harnesses": await asyncio.to_thread(harness_saude.diagnosticar)}
