# Cloud Sync of the Server List Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the PWA's backend list (VPS + home + work) sync across devices via an opt-in,
zero-knowledge hub running on the always-on VPS, so servers are entered once and never re-typed.

**Architecture:** A flag (`CP_SYNC`) turns one backend into a sync hub exposing `/api/sync/*`. The
PWA logs in with a master password; all key derivation and encryption happen in the browser
(PBKDF2→HKDF→AES-GCM). The hub stores only salt + an auth verifier + the ciphertext blob, never the
password or the backend tokens. The existing `localStorage` multi-server layer is hydrated from the
vault on login and pushed back on every mutation.

**Tech Stack:** Backend FastAPI + Python `hashlib`/`hmac` (no new deps). Frontend Svelte 5 +
WebCrypto (`crypto.subtle`, no new runtime deps); vitest added as a dev dependency for unit tests.

## Global Constraints

- Backend default port `:8765` (`settings.port`, env `CP_PORT`); PWA front on `:5173`. Sync routes
  are same-origin via the front's reverse proxy on the VPS.
- Zero new **runtime** dependencies. Backend uses stdlib `hashlib`/`hmac`/`secrets`; frontend uses
  native WebCrypto. `vitest` is dev-only.
- Crypto parameters, copied verbatim from the spec:
  `PBKDF2-SHA256, iterations=600000, dkLen=32`; `HKDF-SHA256` with `info="cp-auth"` and `info="cp-enc"`;
  `AES-256-GCM`; server verifier `PBKDF2-SHA256(authHash, verifier_salt, 200000)`.
- Settings env prefix is `CP_` (pydantic `env_prefix="CP_"`). New settings: `sync`, `sync_bootstrap`,
  `sync_data`, `sync_session_secret`.
- Routes follow the existing pattern `@app.<verb>("/api/...", dependencies=[Depends(...)])`; the sync
  router mounts only when `settings.sync` is true.
- All comments/code/identifiers in English; commit messages conventional, no Claude trailer.

---

## File Structure

**Backend**
- `backend/app/config.py` (modify) — add the four `sync*` settings.
- `backend/app/sync.py` (create) — vault file I/O, crypto verifier, session cookie, rate limiter,
  and the `/api/sync` `APIRouter`. One focused module.
- `backend/app/api.py` (modify) — mount `sync_router` when `settings.sync`.
- `backend/tests/test_sync.py` (create) — endpoint + crypto-verifier tests via FastAPI `TestClient`.

**Frontend**
- `frontend/src/lib/url.ts` (create) — `normalizeBaseUrl` pure helper (also fixes the missing-scheme bug).
- `frontend/src/lib/sync.ts` (create) — WebCrypto KDF/encrypt/decrypt + `/api/sync` fetch client.
- `frontend/src/lib/auth.ts` (modify) — use `normalizeBaseUrl`; add `setServers`/`onServersChanged`
  hydrate+push hooks.
- `frontend/src/screens/Login.svelte` (modify) — render the sync login/register flow when enabled.
- `frontend/src/screens/Chat.svelte` or `frontend/src/App.svelte` (modify) — boot probe of sync
  status, route to the right login mode, push on mutations.
- `frontend/src/lib/url.test.ts`, `frontend/src/lib/sync.test.ts` (create) — vitest unit tests.
- `frontend/package.json`, `frontend/vitest.config.ts` (modify/create) — add vitest.

---

## Task 1: Frontend — `normalizeBaseUrl` helper + vitest setup (fixes missing-scheme bug)

Independent of sync. Fixes the current "Failed to fetch" when the user types `localhost:8765`
without `http://`, by normalizing the base URL at the single chokepoint (`addServer`).

**Files:**
- Create: `frontend/src/lib/url.ts`
- Create: `frontend/src/lib/url.test.ts`
- Create: `frontend/vitest.config.ts`
- Modify: `frontend/package.json` (add `"test": "vitest run"` script + `vitest` devDependency)
- Modify: `frontend/src/lib/auth.ts:105-128` (call `normalizeBaseUrl` inside `addServer`)

**Interfaces:**
- Produces: `normalizeBaseUrl(raw: string): string` — trims, prepends `http://` when no
  `scheme://` prefix is present, strips trailing slashes. Empty input returns `''`.

- [ ] **Step 1: Add vitest dev dependency and test script**

Run:
```bash
cd frontend && npm i -D vitest
```
Then edit `frontend/package.json` `scripts` to add (next to `"check"`):
```json
"test": "vitest run"
```

- [ ] **Step 2: Create the vitest config**

Create `frontend/vitest.config.ts`:
```ts
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node', // pure-function + WebCrypto units; no DOM needed
    include: ['src/**/*.test.ts'],
  },
});
```

- [ ] **Step 3: Write the failing test**

Create `frontend/src/lib/url.test.ts`:
```ts
import { describe, it, expect } from 'vitest';
import { normalizeBaseUrl } from './url';

describe('normalizeBaseUrl', () => {
  it('prepends http:// when scheme is missing', () => {
    expect(normalizeBaseUrl('localhost:8765')).toBe('http://localhost:8765');
  });
  it('keeps an explicit http scheme', () => {
    expect(normalizeBaseUrl('http://192.168.0.5:8765')).toBe('http://192.168.0.5:8765');
  });
  it('keeps an explicit https scheme', () => {
    expect(normalizeBaseUrl('https://casa.ts.net')).toBe('https://casa.ts.net');
  });
  it('trims whitespace and trailing slashes', () => {
    expect(normalizeBaseUrl('  http://h:1/  ')).toBe('http://h:1');
  });
  it('returns empty string for empty input', () => {
    expect(normalizeBaseUrl('   ')).toBe('');
  });
});
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/lib/url.test.ts`
Expected: FAIL — `Cannot find module './url'`.

- [ ] **Step 5: Implement `normalizeBaseUrl`**

Create `frontend/src/lib/url.ts`:
```ts
// Aceita o usuario digitar "localhost:8765" sem esquema: fetch leria "localhost:" como protocolo e
// quebraria ("Failed to fetch"). Prefixa http:// quando nao ha "scheme://", e tira barra final pra
// casar com a normalizacao de addServer (dedup por baseUrl).
export function normalizeBaseUrl(raw: string): string {
  const s = raw.trim();
  if (!s) return '';
  const withScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(s) ? s : `http://${s}`;
  return withScheme.replace(/\/+$/, '');
}
```

- [ ] **Step 6: Wire it into `addServer`**

In `frontend/src/lib/auth.ts`, import at the top (after the existing imports):
```ts
import { normalizeBaseUrl } from './url';
```
Then in `addServer` (currently `frontend/src/lib/auth.ts:105`), normalize the URL before dedup/store.
Change:
```ts
  const norm = (u: string) => u.replace(/\/+$/, '');
  const list = readServers();
  const i = list.findIndex((s) => norm(s.baseUrl) === norm(baseUrl));
```
to:
```ts
  baseUrl = normalizeBaseUrl(baseUrl);
  const norm = (u: string) => u.replace(/\/+$/, '');
  const list = readServers();
  const i = list.findIndex((s) => norm(s.baseUrl) === norm(baseUrl));
```
`labelFor(baseUrl)` keeps working since the URL now parses.

- [ ] **Step 7: Run tests + typecheck**

Run: `cd frontend && npx vitest run src/lib/url.test.ts && npm run check`
Expected: tests PASS; `check` reports 0 errors.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/lib/url.ts frontend/src/lib/url.test.ts frontend/vitest.config.ts frontend/package.json frontend/package-lock.json frontend/src/lib/auth.ts
git commit -m "fix(login): normalize server URL without scheme; add vitest"
```

---

## Task 2: Backend — sync settings

**Files:**
- Modify: `backend/app/config.py:82-102` (add settings to the `Settings` class)
- Test: covered indirectly by Task 3+ (`TestClient` reads these). No standalone test — trivial config.

**Interfaces:**
- Produces: `settings.sync: bool`, `settings.sync_bootstrap: str`, `settings.sync_data: Path`,
  `settings.sync_session_secret: str`.

- [ ] **Step 1: Add the settings**

In `backend/app/config.py`, inside `class Settings`, after the `vapid_subject` line, add:
```python
    # Cloud sync hub (opt-in). CP_SYNC=1 turns THIS backend into the sync hub: it mounts /api/sync/*.
    # Stores only salt + auth verifier + ciphertext (zero-knowledge; tokens are encrypted client-side).
    sync: bool = False
    sync_bootstrap: str = ""        # CP_SYNC_BOOTSTRAP: one-time secret to gate first registration
    sync_data: Path = Path.home() / ".claude-pocket" / "sync-vault.json"
    sync_session_secret: str = ""   # CP_SYNC_SESSION_SECRET: HMAC key for the session cookie; empty -> random at boot
```

- [ ] **Step 2: Verify it imports**

Run: `cd backend && uv run python -c "from app.config import settings; print(settings.sync, settings.sync_data)"`
Expected: prints `False <home>/.claude-pocket/sync-vault.json`.

- [ ] **Step 3: Commit**

```bash
git add backend/app/config.py
git commit -m "feat(sync): add CP_SYNC settings"
```

---

## Task 3: Backend — vault store, crypto verifier, registration

**Files:**
- Create: `backend/app/sync.py`
- Create: `backend/tests/test_sync.py`

**Interfaces:**
- Consumes: `settings.sync_data`, `settings.sync_bootstrap` (Task 2).
- Produces (module functions used by later steps and Task 4/5):
  - `load_vault() -> dict | None` — parsed JSON or `None` if no file.
  - `save_vault(v: dict) -> None` — atomic write (temp + `os.replace`), creates parent dir.
  - `is_registered() -> bool`
  - `make_verifier(auth_hash: str, verifier_salt: bytes) -> str` — base64 of
    `PBKDF2-SHA256(auth_hash.encode(), verifier_salt, 200_000, 32)`.
  - `register(...)`, `verify_credentials(user, auth_hash) -> bool`, `sync_router: APIRouter`.

- [ ] **Step 1: Write failing tests for register + verifier**

Create `backend/tests/test_sync.py`:
```python
import base64
import importlib

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def client(tmp_path, monkeypatch):
    # Point the vault at a temp file and enable sync BEFORE importing the app, so the router mounts.
    monkeypatch.setenv("CP_SYNC", "1")
    monkeypatch.setenv("CP_SYNC_BOOTSTRAP", "boot-secret")
    monkeypatch.setenv("CP_SYNC_DATA", str(tmp_path / "vault.json"))
    monkeypatch.setenv("CP_SYNC_SESSION_SECRET", "test-session-secret")
    import app.config as config
    importlib.reload(config)
    import app.sync as sync
    importlib.reload(sync)
    import app.api as api
    importlib.reload(api)
    return TestClient(api.app)


# A fake client-derived pair. The server never derives these; it only stores/compares.
SALT = base64.b64encode(b"0123456789abcdef").decode()
AUTH = base64.b64encode(b"auth-hash-32-bytes-padding-here!").decode()


def test_status_unregistered(client):
    r = client.get("/api/sync/status")
    assert r.status_code == 200
    assert r.json() == {"enabled": True, "registered": False}


def test_register_requires_bootstrap(client):
    r = client.post("/api/sync/register",
                    json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "wrong"})
    assert r.status_code == 403


def test_register_once_then_locked(client):
    ok = client.post("/api/sync/register",
                     json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "boot-secret"})
    assert ok.status_code == 200
    again = client.post("/api/sync/register",
                        json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "boot-secret"})
    assert again.status_code == 403
    assert client.get("/api/sync/status").json()["registered"] is True
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd backend && uv run pytest tests/test_sync.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'app.sync'`.

- [ ] **Step 3: Implement the store + verifier + register/status routes**

Create `backend/app/sync.py`:
```python
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

from app.config import settings

# ── Zero-knowledge sync hub ──────────────────────────────────────────────────────────────────
# The browser derives masterKey = PBKDF2(password, salt); from it, authHash (sent here) and encKey
# (NEVER sent). We store salt + a verifier of authHash + the AES-GCM ciphertext of the server list.
# We can recover neither the password nor the backend tokens.

_PBKDF2_VERIFIER_ITERS = 200_000


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


def require_session(request: Request) -> str:
    user = verify_session(request.cookies.get(COOKIE_NAME))
    if not user:
        raise HTTPException(status_code=401, detail="unauthorized")
    return user


# ── Login rate limiter (in-memory) ───────────────────────────────────────────────────────────
# ponytail: per-process dict, fine for one user; reset on restart. Not a distributed limiter.
_FAILS: dict[str, list[float]] = {}
_RL_WINDOW = 15 * 60
_RL_MAX = 5


def rate_limited(ip: str) -> bool:
    now = time.time()
    hits = [t for t in _FAILS.get(ip, []) if now - t < _RL_WINDOW]
    _FAILS[ip] = hits
    return len(hits) >= _RL_MAX


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
```

- [ ] **Step 4: Mount the router (needed for tests to hit the app)**

In `backend/app/api.py`, add the import near the other `from app...` imports:
```python
from app.sync import sync_router
```
Then immediately after the CORS `add_middleware(...)` block (around line 117), add:
```python
if settings.sync:
    app.include_router(sync_router)
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd backend && uv run pytest tests/test_sync.py -v`
Expected: the three tests PASS.

- [ ] **Step 6: Commit**

```bash
git add backend/app/sync.py backend/app/api.py backend/tests/test_sync.py
git commit -m "feat(sync): vault store, auth verifier, bootstrap-gated registration"
```

---

## Task 4: Backend — prelogin, login/logout, vault GET/PUT with rev

**Files:**
- Modify: `backend/app/sync.py` (append routes)
- Modify: `backend/tests/test_sync.py` (append tests)

**Interfaces:**
- Consumes: everything from Task 3.
- Produces routes: `GET /api/sync/prelogin`, `POST /api/sync/login`, `POST /api/sync/logout`,
  `GET /api/sync/vault`, `PUT /api/sync/vault`.

- [ ] **Step 1: Write failing tests for login + vault round-trip + stale rev**

Append to `backend/tests/test_sync.py`:
```python
def _register(client):
    client.post("/api/sync/register",
                json={"user": "j", "salt": SALT, "auth_hash": AUTH, "bootstrap": "boot-secret"})


def test_prelogin_returns_salt(client):
    _register(client)
    r = client.get("/api/sync/prelogin", params={"user": "j"})
    assert r.status_code == 200
    assert r.json()["salt"] == SALT
    assert r.json()["iterations"] == 600000


def test_login_good_and_bad(client):
    _register(client)
    bad = client.post("/api/sync/login", json={"user": "j", "auth_hash": "deadbeef"})
    assert bad.status_code == 401
    ok = client.post("/api/sync/login", json={"user": "j", "auth_hash": AUTH})
    assert ok.status_code == 200
    assert "cp_sync" in ok.cookies


def test_vault_round_trip_and_stale_rev(client):
    _register(client)
    client.post("/api/sync/login", json={"user": "j", "auth_hash": AUTH})
    empty = client.get("/api/sync/vault")
    assert empty.status_code == 200
    assert empty.json() == {"enc_blob": None, "rev": 0}

    blob = {"iv": "aXY=", "data": "ZGF0YQ=="}
    put = client.put("/api/sync/vault", json={"enc_blob": blob, "base_rev": 0})
    assert put.status_code == 200
    assert put.json()["rev"] == 1

    got = client.get("/api/sync/vault")
    assert got.json() == {"enc_blob": blob, "rev": 1}

    stale = client.put("/api/sync/vault", json={"enc_blob": blob, "base_rev": 0})
    assert stale.status_code == 409
    assert stale.json()["detail"]["rev"] == 1


def test_vault_requires_session(client):
    _register(client)
    assert client.get("/api/sync/vault").status_code == 401
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd backend && uv run pytest tests/test_sync.py -v -k "prelogin or login or vault"`
Expected: FAIL — 404 on the new routes (not yet defined).

- [ ] **Step 3: Implement the routes**

Append to `backend/app/sync.py`:
```python
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
    return {"salt": salt, "iterations": 600000}


@sync_router.post("/login")
def login(body: LoginBody, request: Request, response: Response) -> dict:
    ip = request.client.host if request.client else "?"
    if rate_limited(ip):
        raise HTTPException(status_code=429, detail="too many attempts")
    if not verify_credentials(body.user, body.auth_hash):
        record_fail(ip)
        raise HTTPException(status_code=401, detail="unauthorized")
    secure = request.url.scheme == "https"
    response.set_cookie(
        COOKIE_NAME, sign_session(body.user),
        max_age=_SESSION_TTL, httponly=True, samesite="lax", secure=secure, path="/",
    )
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
```

- [ ] **Step 4: Run the full sync test file**

Run: `cd backend && uv run pytest tests/test_sync.py -v`
Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add backend/app/sync.py backend/tests/test_sync.py
git commit -m "feat(sync): prelogin, session login/logout, vault GET/PUT with rev conflict"
```

---

## Task 5: Frontend — `lib/sync.ts` crypto + API client

**Files:**
- Create: `frontend/src/lib/sync.ts`
- Create: `frontend/src/lib/sync.test.ts`

**Interfaces:**
- Consumes: `Server` type from `./auth`.
- Produces:
  - `deriveKeys(password: string, saltB64: string, iterations: number): Promise<{authHash: string; encKey: CryptoKey}>`
  - `encryptList(encKey: CryptoKey, servers: Server[]): Promise<{iv: string; data: string}>`
  - `decryptList(encKey: CryptoKey, blob: {iv: string; data: string}): Promise<Server[]>`
  - `syncStatus(): Promise<{enabled: boolean; registered: boolean} | null>` — null if route absent.
  - `prelogin(user): Promise<{salt: string; iterations: number}>`
  - `register(user, password, bootstrap): Promise<void>`
  - `login(user, password): Promise<CryptoKey>` — returns the `encKey` for the session.
  - `logout(): Promise<void>`
  - `getVault(): Promise<{enc_blob: {iv:string;data:string} | null; rev: number}>`
  - `putVault(blob, baseRev): Promise<{rev: number} | {conflict: {enc_blob: any; rev: number}}>`

- [ ] **Step 1: Write the failing crypto round-trip test**

Create `frontend/src/lib/sync.test.ts`:
```ts
import { describe, it, expect } from 'vitest';
import { webcrypto } from 'node:crypto';
import { deriveKeys, encryptList, decryptList } from './sync';

// Node 20+ exposes WebCrypto at globalThis.crypto; ensure it for the module under test.
if (!globalThis.crypto) (globalThis as any).crypto = webcrypto;

describe('sync crypto', () => {
  it('round-trips a server list through derive/encrypt/decrypt', async () => {
    const salt = btoa('0123456789abcdef');
    const { authHash, encKey } = await deriveKeys('hunter2', salt, 600000);
    expect(typeof authHash).toBe('string');
    expect(authHash.length).toBeGreaterThan(0);

    const servers = [{ id: 'a', label: 'casa', baseUrl: 'http://h:1', token: 't1' }];
    const blob = await encryptList(encKey, servers);
    expect(blob.iv).toBeTruthy();
    expect(blob.data).toBeTruthy();

    const out = await decryptList(encKey, blob);
    expect(out).toEqual(servers);
  });

  it('derives the same authHash for the same password+salt', async () => {
    const salt = btoa('0123456789abcdef');
    const a = await deriveKeys('pw', salt, 600000);
    const b = await deriveKeys('pw', salt, 600000);
    expect(a.authHash).toBe(b.authHash);
  });

  it('produces a different authHash for a different password', async () => {
    const salt = btoa('0123456789abcdef');
    const a = await deriveKeys('pw1', salt, 600000);
    const b = await deriveKeys('pw2', salt, 600000);
    expect(a.authHash).not.toBe(b.authHash);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/lib/sync.test.ts`
Expected: FAIL — `Cannot find module './sync'`.

- [ ] **Step 3: Implement the crypto + client**

Create `frontend/src/lib/sync.ts`:
```ts
import type { Server } from './auth';

// Zero-knowledge: the password never leaves the browser. From PBKDF2(masterKey) we split two HKDF
// branches — authHash (sent to the hub) and encKey (stays here, encrypts the server list).
const enc = new TextEncoder();
const dec = new TextDecoder();

function b64(buf: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(buf)));
}
function unb64(s: string): Uint8Array {
  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
}

export async function deriveKeys(
  password: string,
  saltB64: string,
  iterations: number,
): Promise<{ authHash: string; encKey: CryptoKey }> {
  const salt = unb64(saltB64);
  const baseKey = await crypto.subtle.importKey('raw', enc.encode(password), 'PBKDF2', false, ['deriveBits']);
  const masterBits = await crypto.subtle.deriveBits(
    { name: 'PBKDF2', salt, iterations, hash: 'SHA-256' }, baseKey, 256,
  );
  const masterKey = await crypto.subtle.importKey('raw', masterBits, 'HKDF', false, ['deriveBits', 'deriveKey']);
  const authBits = await crypto.subtle.deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(0), info: enc.encode('cp-auth') }, masterKey, 256,
  );
  const encKey = await crypto.subtle.deriveKey(
    { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(0), info: enc.encode('cp-enc') },
    masterKey, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt'],
  );
  return { authHash: b64(authBits), encKey };
}

export async function encryptList(encKey: CryptoKey, servers: Server[]): Promise<{ iv: string; data: string }> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const pt = enc.encode(JSON.stringify(servers));
  const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, encKey, pt); // ct includes the GCM tag
  return { iv: b64(iv.buffer), data: b64(ct) };
}

export async function decryptList(encKey: CryptoKey, blob: { iv: string; data: string }): Promise<Server[]> {
  const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: unb64(blob.iv) }, encKey, unb64(blob.data));
  return JSON.parse(dec.decode(pt));
}

// ── API client (same-origin; the front's reverse proxy forwards /api to the co-located backend) ──
async function jf(path: string, init?: RequestInit): Promise<Response> {
  return fetch(path, { credentials: 'include', headers: { 'Content-Type': 'application/json' }, ...init });
}

export async function syncStatus(): Promise<{ enabled: boolean; registered: boolean } | null> {
  try {
    const r = await jf('/api/sync/status');
    if (!r.ok) return null;
    return await r.json();
  } catch {
    return null; // route absent / network -> sync disabled
  }
}

export async function prelogin(user: string): Promise<{ salt: string; iterations: number }> {
  const r = await jf(`/api/sync/prelogin?user=${encodeURIComponent(user)}`);
  if (!r.ok) throw new Error('prelogin failed');
  return await r.json();
}

export async function register(user: string, password: string, bootstrap: string): Promise<void> {
  const salt = b64(crypto.getRandomValues(new Uint8Array(16)).buffer);
  const { authHash } = await deriveKeys(password, salt, 600000);
  const r = await jf('/api/sync/register', {
    method: 'POST', body: JSON.stringify({ user, salt, auth_hash: authHash, bootstrap }),
  });
  if (!r.ok) throw new Error((await r.json()).detail ?? 'register failed');
}

export async function login(user: string, password: string): Promise<CryptoKey> {
  const { salt, iterations } = await prelogin(user);
  const { authHash, encKey } = await deriveKeys(password, salt, iterations);
  const r = await jf('/api/sync/login', {
    method: 'POST', body: JSON.stringify({ user, auth_hash: authHash }),
  });
  if (!r.ok) throw new Error(r.status === 429 ? 'muitas tentativas' : 'usuário ou senha inválidos');
  return encKey;
}

export async function logout(): Promise<void> {
  await jf('/api/sync/logout', { method: 'POST' });
}

export async function getVault(): Promise<{ enc_blob: { iv: string; data: string } | null; rev: number }> {
  const r = await jf('/api/sync/vault');
  if (!r.ok) throw new Error('vault read failed');
  return await r.json();
}

export async function putVault(
  blob: { iv: string; data: string } | null,
  baseRev: number,
): Promise<{ rev: number } | { conflict: { enc_blob: any; rev: number } }> {
  const r = await jf('/api/sync/vault', {
    method: 'PUT', body: JSON.stringify({ enc_blob: blob, base_rev: baseRev }),
  });
  if (r.status === 409) return { conflict: (await r.json()).detail };
  if (!r.ok) throw new Error('vault write failed');
  return await r.json();
}
```

- [ ] **Step 4: Run the crypto tests**

Run: `cd frontend && npx vitest run src/lib/sync.test.ts`
Expected: all three tests PASS.

- [ ] **Step 5: Typecheck**

Run: `cd frontend && npm run check`
Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/sync.ts frontend/src/lib/sync.test.ts
git commit -m "feat(sync): client-side zero-knowledge crypto + /api/sync client"
```

---

## Task 6: Frontend — `auth.ts` hydrate + push hooks

**Files:**
- Modify: `frontend/src/lib/auth.ts` (add `setServers` + `onServersChanged` + `notifyChanged` calls)

**Interfaces:**
- Consumes: existing `readServers`/`writeServers`/`ACTIVE_KEY`.
- Produces:
  - `setServers(list: Server[]): void` — overwrite the whole list (hydrate from the vault); preserves
    the active id if still present, else picks the first; does NOT fire `notifyChanged`.
  - `onServersChanged(cb: () => void): void` — register a single listener fired after
    `addServer`/`removeServer`/`renameServer`.

- [ ] **Step 1: Add the hydrate + listener API**

In `frontend/src/lib/auth.ts`, after `writeServers`, add:
```ts
// Listener unico: o sync registra aqui pra empurrar pro hub apos qualquer mutacao local.
let _changed: (() => void) | null = null;
export function onServersChanged(cb: () => void): void {
  _changed = cb;
}
function notifyChanged(): void {
  if (_changed) _changed();
}

// Sobrescreve a lista inteira (hidratacao a partir do vault decifrado). Mantem o ativo se ainda
// existir, senao cai pro primeiro. NAO dispara notifyChanged (veio do hub, nao re-empurrar).
export function setServers(list: Server[]): void {
  writeServers(list);
  const active = localStorage.getItem(ACTIVE_KEY);
  if (!active || !list.some((s) => s.id === active)) {
    if (list[0]) localStorage.setItem(ACTIVE_KEY, list[0].id);
    else localStorage.removeItem(ACTIVE_KEY);
  }
}
```

- [ ] **Step 2: Fire the listener from the three mutators**

In `addServer`, before `return { id, existed };` (after `syncCookie(token);`), add:
```ts
  notifyChanged();
```
In `renameServer`, after its `writeServers(list);`, add:
```ts
  notifyChanged();
```
In `removeServer`, as the last line before the function closes, add:
```ts
  notifyChanged();
```

- [ ] **Step 3: Typecheck + existing tests still pass**

Run: `cd frontend && npm run check && npx vitest run`
Expected: 0 errors; url + sync tests still PASS.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/auth.ts
git commit -m "feat(sync): setServers hydrate + onServersChanged push hook in auth store"
```

---

## Task 7: Frontend — Login sync mode + boot wiring + push

**Files:**
- Modify: `frontend/src/screens/Login.svelte` (add the sync login/register UI + flow)
- Modify: the gate component that renders `<Login>` (confirm via grep below)

NOTE: Before editing, find where `Login` is rendered and the logged-in gate lives:
`grep -rn "Login" frontend/src/*.svelte frontend/src/screens frontend/src/App.svelte 2>/dev/null`.
Wire the boot probe and post-login hydrate at that gate. Steps name `Chat.svelte` per the SSE docs;
adjust to the real host if different.

**Interfaces:**
- Consumes: `syncStatus`, `register`, `login`, `getVault`, `decryptList`, `putVault`, `encryptList`,
  `logout` (Task 5); `setServers`, `listServers`, `onServersChanged`, `clearCredentials` (Tasks 6/existing).
- Produces: a session-lived in-memory `encKey` held by the gate; the push callback.

- [ ] **Step 1: Add sync state + flow to Login.svelte**

In `frontend/src/screens/Login.svelte` `<script>`, add import:
```ts
import { syncStatus, register as syncRegister, login as syncLogin } from '../lib/sync';
```
Extend `Props`:
```ts
interface Props {
  onLogin: () => void;
  onSyncLogin?: (encKey: CryptoKey) => void; // called in sync mode with the in-memory key
}
let { onLogin, onSyncLogin }: Props = $props();
```
Add state + handlers:
```ts
let syncMode = $state<null | { registered: boolean }>(null); // null = disabled / not yet probed
let user = $state('');
let password = $state('');
let bootstrap = $state('');
let syncLoading = $state(false);
let syncError = $state('');

onMount(async () => {
  const s = await syncStatus();
  if (s?.enabled) syncMode = { registered: s.registered };
});

async function doSyncSubmit(e: SubmitEvent) {
  e.preventDefault();
  syncLoading = true;
  syncError = '';
  try {
    if (syncMode && !syncMode.registered) {
      await syncRegister(user.trim(), password, bootstrap.trim());
    }
    const encKey = await syncLogin(user.trim(), password);
    onSyncLogin?.(encKey);
    onLogin();
  } catch (err) {
    syncError = err instanceof Error ? err.message : 'falha';
  } finally {
    syncLoading = false;
  }
}
```
NOTE: the file already has a `onMount` (for QR `?token=`). Merge both into one `onMount` (call the
existing token-pickup logic, then the `syncStatus()` probe) rather than declaring `onMount` twice.

- [ ] **Step 2: Render the sync form when enabled**

In `Login.svelte` markup, just inside `<div class="login-content">` after the
`<p class="app-tagline">…</p>`, wrap the existing token form:
```svelte
{#if syncMode}
  <form onsubmit={doSyncSubmit} class="login-form">
    <div class="field">
      <label class="field-label" for="sync-user">Usuário</label>
      <input id="sync-user" class="field-input" bind:value={user} autocomplete="username" autocapitalize="off" spellcheck={false} required />
    </div>
    <div class="field">
      <label class="field-label" for="sync-pass">Senha</label>
      <input id="sync-pass" type="password" class="field-input" bind:value={password} autocomplete="current-password" required />
    </div>
    {#if !syncMode.registered}
      <div class="field">
        <label class="field-label" for="sync-boot">Token de ativação (primeiro acesso)</label>
        <input id="sync-boot" type="password" class="field-input" bind:value={bootstrap} required />
      </div>
    {/if}
    {#if syncError}<p class="error-msg" role="alert">{syncError}</p>{/if}
    <button type="submit" class="connect-btn" disabled={syncLoading || !user.trim() || !password}>
      {syncLoading ? 'Entrando…' : (syncMode.registered ? 'Entrar' : 'Criar acesso')}
    </button>
  </form>
{:else}
```
Then close with `{/if}` immediately AFTER the existing `<form onsubmit={handleSubmit} …>…</form>`
block (the URL+token form becomes the sync-off branch).

- [ ] **Step 3: Wire the boot probe + hydrate + push at the gate**

In the gate component (confirm via the NOTE grep), add:
```ts
import { getVault, decryptList, encryptList, putVault, logout as syncLogout } from '../lib/sync';
import { setServers, listServers, onServersChanged, clearCredentials } from '../lib/auth';

let encKey: CryptoKey | null = null;
let vaultRev = 0;

// After a successful sync login: pull the vault, decrypt, hydrate the local list, wire the push.
async function onSyncLogin(key: CryptoKey) {
  encKey = key;
  const { enc_blob, rev } = await getVault();
  vaultRev = rev;
  if (enc_blob) setServers(await decryptList(key, enc_blob));
  onServersChanged(async () => {
    if (!encKey) return;
    let res = await putVault(await encryptList(encKey, listServers()), vaultRev);
    if ('conflict' in res) {           // a stale rev: adopt the hub's rev and retry once
      vaultRev = res.conflict.rev;
      res = await putVault(await encryptList(encKey, listServers()), vaultRev);
    }
    if ('rev' in res) vaultRev = res.rev;
  });
}
```
Pass it to the login: `<Login onLogin={...} onSyncLogin={onSyncLogin} />`. In the logout handler, when
`encKey` is set, also `await syncLogout()`, then `clearCredentials()` and `encKey = null` so a
logged-out device keeps nothing locally.

- [ ] **Step 4: Typecheck**

Run: `cd frontend && npm run check`
Expected: 0 errors.

- [ ] **Step 5: Manual smoke (hub on)**

Start the hub:
```bash
cd backend && CP_SYNC=1 CP_SYNC_BOOTSTRAP=test-boot CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=127.0.0.1 uv run python -m app.main
```
Serve the front (`npm --prefix frontend run dev`), open it. Verify: first load shows "Criar acesso";
register with `test-boot`; add a server; reload → still present; `~/.claude-pocket/sync-vault.json`
exists with a non-null `enc_blob` and `rev >= 1`; confirm no plaintext token is visible in the file.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/screens/Login.svelte frontend/src/screens/Chat.svelte
git commit -m "feat(sync): login/register UI + vault hydrate-on-login and push-on-change"
```

---

## Task 8: Docs

**Files:**
- Modify: `docs/USAGE.md` (a "Sync na nuvem (opcional)" section)
- Modify: `README.md` (mention the `CP_SYNC*` envs in the run/config table)

- [ ] **Step 1: Document the flag, setup, and zero-knowledge model**

Add to `docs/USAGE.md`: what `CP_SYNC=1` does, setting `CP_SYNC_BOOTSTRAP`, the first-run "Criar
acesso" step, that the password is the only key (no reset), and that the hub must be served over
HTTPS. Add `CP_SYNC`, `CP_SYNC_BOOTSTRAP`, `CP_SYNC_DATA`, `CP_SYNC_SESSION_SECRET` to the README env
list.

- [ ] **Step 2: Commit**

```bash
git add docs/USAGE.md README.md
git commit -m "docs(sync): document the opt-in cloud sync hub"
```

---

## Self-Review

**Spec coverage:**
- Activation flag → Task 2 (settings) + Task 3 (`/status`, mount gating) + Task 7 (front probe). ✅
- Zero-knowledge crypto (PBKDF2→HKDF→AES-GCM) → Task 5; server verifier → Task 3. ✅
- First-run registration anti-hijack (bootstrap, single-shot) → Task 3. ✅
- Session (signed cookie, rate limit) → Task 4. ✅
- Vault GET/PUT + rev/409 → Task 4 (backend) + Task 7 (client retry). ✅
- One-file atomic storage → Task 3 (`save_vault`). ✅
- Frontend hydrate/push without disrupting Sidebar → Task 6 + Task 7. ✅
- Missing-scheme root fix (user-requested addition) → Task 1. ✅
- Security checklist (no enumeration, HttpOnly/Secure, ciphertext-only at rest) → Tasks 3/4 + Step 5 smoke. ✅
- Docs → Task 8. ✅

**Placeholder scan:** No TBD/TODO; every code step shows full code. Task 7 names `Chat.svelte` but
flags that the gate host must be confirmed by grep — a verification instruction, not a placeholder.

**Type consistency:** `deriveKeys`/`encryptList`/`decryptList`/`putVault`/`getVault` signatures match
between Task 5 (definition) and Task 7 (use). Blob shape `{iv, data}` is consistent across `sync.ts`,
`test_sync.py`, and the backend (opaque `dict`). `authHash` is base64 everywhere.
`onServersChanged`/`setServers` names match between Task 6 and Task 7. `iterations=600000` matches
between front `deriveKeys` calls and backend `prelogin`.
