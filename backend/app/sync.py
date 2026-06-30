import base64
import hashlib
import hmac
import json
import os
import secrets
import time
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException, Request, Response
from pydantic import BaseModel

from app.config import settings, _LOOPBACK

# ── Zero-knowledge sync hub ──────────────────────────────────────────────────────────────────
# The browser derives masterKey = PBKDF2(password, salt); from it, authHash (sent here) and encKey
# (NEVER sent). We store salt + a verifier of authHash + the AES-GCM ciphertext of the server list.
# We can recover neither the password nor the backend tokens.

_PBKDF2_VERIFIER_ITERS = 200_000
PBKDF2_ITERATIONS = 600_000  # master-key derivation iters; MUST match the frontend constant of the same name


def _data_path() -> Path:
    return Path(settings.sync_data)


def load_vault() -> dict | None:
    try:
        return json.loads(_data_path().read_text())
    except FileNotFoundError:
        return None
    except (OSError, json.JSONDecodeError):
        return None


def save_vault(v: dict) -> None:
    p = _data_path()
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(p.suffix + ".tmp")
    tmp.write_text(json.dumps(v))
    os.replace(tmp, p)  # atomic


def is_registered() -> bool:
    return load_vault() is not None


def make_verifier(auth_hash: str, verifier_salt: bytes) -> str:
    dk = hashlib.pbkdf2_hmac("sha256", auth_hash.encode(), verifier_salt, _PBKDF2_VERIFIER_ITERS, 32)
    return base64.b64encode(dk).decode()


def verify_credentials(user: str, auth_hash: str) -> bool:
    v = load_vault()
    if not v or v.get("user") != user:
        return False
    vsalt = base64.b64decode(v["verifier_salt"])
    expect = make_verifier(auth_hash, vsalt)
    return hmac.compare_digest(expect, v["auth_verifier"])


# ── Session cookie (signed) ──────────────────────────────────────────────────────────────────
_SESSION_TTL = 30 * 24 * 3600  # 30 days
# Empty secret -> random per process (restart logs everyone out; fine for single user).
_SESSION_SECRET = (settings.sync_session_secret or secrets.token_hex(32)).encode()
COOKIE_NAME = "cp_sync"


def sign_session(user: str) -> str:
    exp = int(time.time()) + _SESSION_TTL
    msg = f"{user}.{exp}"
    sig = hmac.new(_SESSION_SECRET, msg.encode(), hashlib.sha256).hexdigest()
    return f"{msg}.{sig}"


def verify_session(cookie: str | None) -> str | None:
    if not cookie:
        return None
    try:
        user, exp_s, sig = cookie.rsplit(".", 2)
    except ValueError:
        return None
    msg = f"{user}.{exp_s}"
    good = hmac.new(_SESSION_SECRET, msg.encode(), hashlib.sha256).hexdigest()
    if not hmac.compare_digest(good, sig):
        return None
    if int(exp_s) < int(time.time()):
        return None
    return user


def _set_session_cookie(response: Response, user: str, request: Request) -> None:
    # Loopback bind = dev over http (Secure off senao o cookie nao gruda); qualquer bind non-loopback
    # = deploy real (HTTPS exigido) -> forca Secure mesmo que o proxy reporte http.
    secure = request.url.scheme == "https" or settings.lan_bind_ip not in _LOOPBACK
    response.set_cookie(
        COOKIE_NAME, sign_session(user),
        max_age=_SESSION_TTL, httponly=True, samesite="lax", secure=secure, path="/",
    )


def require_session(request: Request, response: Response) -> str:
    user = verify_session(request.cookies.get(COOKIE_NAME))
    if not user:
        raise HTTPException(status_code=401, detail="unauthorized")
    _set_session_cookie(response, user, request)  # sliding: renova o prazo a cada request autenticado
    return user


# ── Login rate limiter (in-memory) ───────────────────────────────────────────────────────────
# ponytail: per-process dict, fine for one user; reset on restart. Not a distributed limiter.
# Window/max are configurable (CP_SYNC_RATE_*) so a single user fat-fingering the password isn't
# locked out for long. Keyed by request.client.host -> the REAL client only when uvicorn trusts the
# proxy (forwarded_allow_ips); otherwise it's the proxy IP (a shared bucket) -- hence the loose default.
_FAILS: dict[str, list[float]] = {}


def rate_limited(ip: str) -> bool:
    now = time.time()
    hits = [t for t in _FAILS.get(ip, []) if now - t < settings.sync_rate_window]
    _FAILS[ip] = hits
    return len(hits) >= settings.sync_rate_max


def record_fail(ip: str) -> None:
    _FAILS.setdefault(ip, []).append(time.time())


# ── Routes ───────────────────────────────────────────────────────────────────────────────────
sync_router = APIRouter(prefix="/api/sync")


class RegisterBody(BaseModel):
    user: str
    salt: str       # base64, browser-generated
    auth_hash: str  # base64, browser-derived
    bootstrap: str


@sync_router.get("/status")
def status() -> dict:
    return {"enabled": True, "registered": is_registered()}


@sync_router.post("/register")
def register(body: RegisterBody) -> dict:
    if is_registered():
        raise HTTPException(status_code=403, detail="already registered")
    if not settings.sync_bootstrap or not hmac.compare_digest(body.bootstrap, settings.sync_bootstrap):
        raise HTTPException(status_code=403, detail="bad bootstrap")
    vsalt = secrets.token_bytes(16)
    save_vault({
        "user": body.user,
        "salt": body.salt,
        "verifier_salt": base64.b64encode(vsalt).decode(),
        "auth_verifier": make_verifier(body.auth_hash, vsalt),
        "enc_blob": None,
        "rev": 0,
    })
    return {"ok": True}


class LoginBody(BaseModel):
    user: str
    auth_hash: str


class VaultPutBody(BaseModel):
    enc_blob: dict | None
    base_rev: int


@sync_router.get("/prelogin")
def prelogin(user: str) -> dict:
    # Always return the stored salt + iterations regardless of username, to avoid user enumeration.
    # A wrong user just fails later at /login. If no account yet, return a stable placeholder salt.
    v = load_vault()
    salt = v["salt"] if v else base64.b64encode(b"unregistered----").decode()
    return {"salt": salt, "iterations": PBKDF2_ITERATIONS}


@sync_router.post("/login")
def login(body: LoginBody, request: Request, response: Response) -> dict:
    ip = request.client.host if request.client else "?"
    if rate_limited(ip):
        raise HTTPException(status_code=429, detail="too many attempts")
    if not verify_credentials(body.user, body.auth_hash):
        record_fail(ip)
        raise HTTPException(status_code=401, detail="unauthorized")
    _set_session_cookie(response, body.user, request)
    return {"ok": True}


@sync_router.post("/logout")
def logout(response: Response) -> dict:
    response.delete_cookie(COOKIE_NAME, path="/")
    return {"ok": True}


@sync_router.get("/vault")
def get_vault(user: str = Depends(require_session)) -> dict:
    v = load_vault() or {"enc_blob": None, "rev": 0}
    return {"enc_blob": v.get("enc_blob"), "rev": v.get("rev", 0)}


@sync_router.put("/vault")
def put_vault(body: VaultPutBody, user: str = Depends(require_session)) -> dict:
    v = load_vault()
    if not v:
        raise HTTPException(status_code=409, detail={"enc_blob": None, "rev": 0})
    if body.base_rev != v["rev"]:
        raise HTTPException(status_code=409, detail={"enc_blob": v["enc_blob"], "rev": v["rev"]})
    v["enc_blob"] = body.enc_blob
    v["rev"] += 1
    save_vault(v)
    return {"rev": v["rev"]}
