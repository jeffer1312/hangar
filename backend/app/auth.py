from fastapi import Request, HTTPException
from app.config import settings


def require_auth(request: Request) -> None:
    auth = request.headers.get("Authorization", "")
    token = auth[7:] if auth.startswith("Bearer ") else request.cookies.get("cp_token")
    if token != settings.auth_token:
        raise HTTPException(status_code=401, detail="unauthorized")
