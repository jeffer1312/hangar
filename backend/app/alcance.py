"""Rotas do alcance — por onde este servidor responde (aba Acesso).

A Task 1 só registra o roteador com a listagem vazia. As Tasks 3 e 6 (Lote B) escrevem
aqui dentro, sem tocar em api.py.
"""
from fastapi import APIRouter, Depends

from app.auth import require_auth

alcance_router = APIRouter(prefix="/api/alcance")


@alcance_router.get("", dependencies=[Depends(require_auth)])
def listar_alcance() -> list:
    return []
