from fastapi import Request, HTTPException
from app.config import settings


def require_auth(request: Request) -> None:
    auth = request.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        token = auth[7:]
    else:
        # A SSE (EventSource) nao consegue mandar header Authorization; cross-origin (multi-PC) o
        # cookie tb nao vai (SameSite) -> sobra o ?token= na URL. Aceitar a query e o que faltava
        # (era 401 em /events?token=...). Ordem: header -> query -> cookie (same-origin).
        token = request.query_params.get("token") or request.cookies.get("cp_token")
    if token != settings.auth_token:
        raise HTTPException(status_code=401, detail="unauthorized")
