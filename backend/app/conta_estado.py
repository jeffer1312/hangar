"""Rotas do estado de conta — a lista de contas do servidor (aba Contas).

A Task 1 só registra o roteador com a listagem vazia. As Tasks 4 e 7 (Lote B) escrevem
aqui dentro, sem tocar em api.py.
"""
from fastapi import APIRouter, Depends

from app.auth import require_auth

conta_estado_router = APIRouter(prefix="/api/conta-estado")


@conta_estado_router.get("", dependencies=[Depends(require_auth)])
def listar_contas() -> list:
    return []
