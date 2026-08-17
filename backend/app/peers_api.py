"""Rotas dos peers — máquinas que este servidor alcança (aba Servidores).

A Task 1 só registra o roteador com a listagem vazia. As Tasks 5 e 8 (Lote B) escrevem
aqui dentro, sem tocar em api.py.

Nome do módulo: `peers_api` de propósito — `app/peers.py` já é a lógica de pareamento
cross-server, e o módulo de rota não pode ocupar o nome dela.
"""
from fastapi import APIRouter, Depends

from app.auth import require_auth

peers_router = APIRouter(prefix="/api/peers")


@peers_router.get("", dependencies=[Depends(require_auth)])
def listar_peers() -> list:
    return []
