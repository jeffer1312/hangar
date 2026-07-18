"""Auto-deploy via webhook do GitHub.

Fluxo: push na `main` -> GitHub POST em /api/deploy/github-webhook com header
`X-Hub-Signature-256` (HMAC-SHA256 do corpo cru, chave = CP_DEPLOY_SECRET). Aqui a assinatura
e validada em tempo constante e, se bater E o evento for push na main, dispara (nao-bloqueante)
a unit `claude-cockpit-deploy.service` que faz pull+build+restart num processo INDEPENDENTE.

Por que unit separada e nao rodar o deploy aqui: o deploy reinicia ESTE backend. Rodar inline
mataria o handler no meio da request (GitHub veria timeout/falha). Delegando pra uma oneshot
via `systemctl --user start --no-block`, o handler retorna 202 na hora e o deploy sobrevive ao
restart. Sem require_auth: o GitHub nao manda Bearer; a autenticacao E a assinatura HMAC.
"""
import hashlib
import hmac
import json
import logging
import subprocess

from fastapi import APIRouter, Request, HTTPException

from app.config import settings

_log = logging.getLogger("claude_pocket")

deploy_router = APIRouter()

DEPLOY_UNIT = "claude-cockpit-deploy.service"


def _verify_signature(body: bytes, header: str) -> bool:
    """Compara HMAC-SHA256(body, secret) com o header 'sha256=<hex>' do GitHub, em tempo constante."""
    if not header.startswith("sha256="):
        return False
    expected = hmac.new(settings.deploy_secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header[len("sha256="):])


@deploy_router.post("/api/deploy/github-webhook")
async def github_webhook(request: Request):
    # Desligado por padrao: sem secret configurado o endpoint some (404), nao fica um trigger aberto.
    if not settings.deploy_secret:
        raise HTTPException(status_code=404, detail="not found")

    body = await request.body()
    sig = request.headers.get("X-Hub-Signature-256", "")
    if not _verify_signature(body, sig):
        _log.warning("[deploy] webhook com assinatura invalida (ip=%s)", request.client.host if request.client else "?")
        raise HTTPException(status_code=401, detail="bad signature")

    event = request.headers.get("X-GitHub-Event", "")
    if event == "ping":
        return {"status": "pong"}
    if event != "push":
        return {"status": "ignored", "reason": f"event={event}"}

    try:
        ref = json.loads(body).get("ref", "")
    except (ValueError, AttributeError):
        raise HTTPException(status_code=400, detail="invalid payload")
    if ref != "refs/heads/main":
        return {"status": "ignored", "reason": f"ref={ref}"}

    # --no-block: 'start' de uma oneshot bloquearia ate o ExecStart terminar; aqui so enfileira o job
    # e volta. O processo herda XDG_RUNTIME_DIR/DBUS do backend (user service) -> systemctl --user ok.
    try:
        subprocess.run(
            ["systemctl", "--user", "start", "--no-block", DEPLOY_UNIT],
            check=True, capture_output=True, text=True, timeout=10,
        )
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
        stderr = getattr(e, "stderr", "") or str(e)
        _log.error("[deploy] falha ao disparar %s: %s", DEPLOY_UNIT, stderr)
        raise HTTPException(status_code=500, detail="failed to trigger deploy")

    _log.info("[deploy] push na main aceito -> %s disparado", DEPLOY_UNIT)
    return {"status": "accepted"}
