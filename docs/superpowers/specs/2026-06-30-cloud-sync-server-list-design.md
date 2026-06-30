# Cloud sync of the server list — opt-in zero-knowledge hub

**Date:** 2026-06-30
**Status:** Design — approved verbally, pending written review

## Problem

The PWA stores its backend list (`cp_servers` in `localStorage`: `{id,label,baseUrl,token}[]`)
per-browser, per-origin. So the list does not travel: a server added on the phone is absent on the
desktop, and reinstalling/clearing the PWA wipes it. The user re-enters servers constantly.

The user already runs claude-pocket on three machines — **VPS, home, work** — with the VPS always
on. The VPS can act as a central **sync hub** that holds the canonical list; every device logs in and
receives it.

This is a real trust boundary: the stored list contains the **bearer tokens** of each backend, and a
token drives a live `claude` session (shell-equivalent power) on that machine. The hub must never hold
those tokens in a form it can read.

## Non-goals

- Not a multi-user/account product. Single user, single account. The "login" is one master credential.
- Not a replacement for the current per-server token/QR pairing — that stays as the default (sync OFF).
- No password reset / recovery. Zero-knowledge means the server cannot recover the data; the user owns
  the only key. (Accepted by the user — it is their own password.)
- No separate sync service. The endpoints live in the existing `backend/app`, gated by a flag.

## Approach (chosen)

Option **C** from brainstorming: a central account hub, **opt-in behind a flag**, with
**client-side (zero-knowledge) encryption** of the list. Rejected alternatives: export/import QR
(manual, no auto-sync); server-side at-rest encryption (key on the VPS → full compromise reveals
tokens); plaintext (unacceptable for shell-power tokens).

## Activation flag

- New setting `CP_SYNC` (bool, default `false`) in `backend/app/config.py`. When `true`, the backend
  mounts the `/sync/*` routes and serves a `GET /sync/status` discovery endpoint.
- The PWA, on load, calls `GET /sync/status` **on its own origin** (the origin it was served from —
  the VPS when sync is used). Response `{enabled: bool, registered: bool}`:
  - `enabled:false` → today's behavior unchanged (token/QR multi-server, no login screen).
  - `enabled:true, registered:false` → first-run: show **"Criar acesso"** (set password + bootstrap).
  - `enabled:true, registered:true` → show **"Entrar"** (user + password).
- Sync mode is a property of the **serving origin**, not a per-server toggle. You point the PWA at the
  VPS to use sync.

## Crypto design (zero-knowledge, Bitwarden-style)

All key derivation happens **in the browser**. The password never leaves the client.

```
masterKey  = PBKDF2-SHA256(password, salt, iterations=600_000, dkLen=32)
authHash   = base64( HKDF-SHA256(masterKey, info="cp-auth", len=32) )   # sent to server
encKey     =          HKDF-SHA256(masterKey, info="cp-enc",  len=32)    # NEVER sent
```

- `salt`: 16 random bytes, generated **once** at registration, stored on the hub, returned by
  `prelogin` so any device can re-derive.
- Server stores `auth_verifier = PBKDF2-SHA256(authHash, verifier_salt, 200_000)` (dependency-free in
  Python `hashlib`). It verifies login by recomputing against the submitted `authHash`. It can never
  derive `encKey` from `authHash` (separate HKDF info strings).
- Vault payload: `enc_blob = AES-256-GCM(encKey, iv, plaintext=JSON(serverList))`, stored as
  `{iv, ciphertext, tag}` base64. The IV is random per write.

**Algorithm choice:** PBKDF2-SHA256 and HKDF are native in browser WebCrypto (`crypto.subtle`) and
Python `hashlib` — **zero new dependencies**. AES-GCM is native too.
`# ponytail: PBKDF2 600k is OWASP-acceptable and dep-free; upgrade path is argon2id via hash-wasm
(client) + argon2-cffi (server) if GPU-cracking resistance becomes a concern.`

## First-run registration (anti-hijack)

A fresh hub has no account, so the registration endpoint must not let a stranger claim it first.

- Env `CP_SYNC_BOOTSTRAP` = a one-time secret printed in the VPS logs/config. The "Criar acesso" form
  asks for username, password, and this bootstrap token.
- `POST /sync/register {user, salt, authHash, bootstrap}`:
  - 403 if an account already exists (registration is single-shot).
  - 403 if `bootstrap` ≠ `CP_SYNC_BOOTSTRAP`.
  - else: persist `{user, salt, auth_verifier}` with an empty vault, `rev=0`. Account is now locked.
- Because the browser derives `salt`/`authHash` and sends only those, the server still never sees the
  password → zero-knowledge holds even through registration.

## Session

- `POST /sync/login {user, authHash}` → on match, set a signed session cookie `cp_sync` (HttpOnly,
  SameSite=Lax, Secure when TLS), HMAC-signed with a per-process secret (`CP_SYNC_SESSION_SECRET`, or
  random at boot → logging out all sessions on restart, acceptable for single user). TTL e.g. 30 days,
  sliding.
- Login is **rate-limited** (e.g. 5 attempts / 15 min, in-memory counter) to blunt brute force.
- `POST /sync/logout` → clears the cookie.
- All `/sync/vault` calls require a valid `cp_sync` cookie.

## Vault sync (last-write-wins with revision)

- `GET /sync/prelogin?user=<u>` (unauthenticated) → `{salt, iterations}` so a new device can derive.
  Returns generic 200 with the salt regardless of username validity to avoid user enumeration; an
  invalid user simply fails at `/sync/login`.
- `GET /sync/vault` (auth) → `{enc_blob, rev}` (`enc_blob` null if empty).
- `PUT /sync/vault {enc_blob, base_rev}` (auth):
  - if `base_rev` ≠ stored `rev` → `409 Conflict` with the current `{enc_blob, rev}`. Client decrypts,
    merges (union by normalized `baseUrl`, mirroring `addServer`'s dedup), re-encrypts, retries.
  - else store, `rev += 1`, return new `rev`.
- Single user rarely writes concurrently; `rev` exists to stop a stale phone from clobbering a desktop
  edit. `# ponytail: in-process rev counter + file; fine for one user, not built for many writers.`

## Storage on the VPS

One JSON file, atomic write (temp file + `os.replace`), path from `CP_SYNC_DATA`
(default `~/.claude-pocket/sync-vault.json`). Synthetic example:

```json
{
  "user": "jefferson",
  "salt": "base64(16B)",
  "iterations": 600000,
  "auth_verifier": "base64",
  "verifier_salt": "base64",
  "enc_blob": { "iv": "...", "ciphertext": "...", "tag": "..." },
  "rev": 7
}
```

No database. `# ponytail: single file for a single user; swap to SQLite only if multi-account ever
happens (it is a non-goal).`

## Backend modules

- `backend/app/sync.py` (new): crypto verifier, file load/save, session cookie sign/verify, rate
  limiter, the route handlers. ~one focused module.
- `backend/app/config.py`: add `sync: bool`, `sync_bootstrap`, `sync_data` (path),
  `sync_session_secret` settings.
- `backend/app/api.py`: mount the `/sync` router only when `settings.sync`.
- Tests: `backend/tests/test_sync.py` — register-once, wrong bootstrap rejected, login good/bad,
  prelogin returns salt, vault GET/PUT round-trip, stale `base_rev` → 409, rate limiter trips.

## Frontend modules

- `frontend/src/lib/sync.ts` (new): WebCrypto KDF/encrypt/decrypt, and the `/sync/*` fetch calls.
  Pure functions + a thin client. Self-check: a `demo()` that derives → encrypts → decrypts → asserts
  round-trip equality.
- `frontend/src/lib/auth.ts`: unchanged storage API. Sync only **hydrates** it (writes the decrypted
  list via the existing write path) on login and **pushes** (re-encrypt + PUT) on
  add/remove/rename/select. A small `onServersChanged` hook so the three mutators trigger a push.
- `frontend/src/screens/Login.svelte`: when `GET /sync/status` says enabled, render the
  user/password "Entrar" (or "Criar acesso") flow instead of (or above) the URL+token form. The
  URL+token form remains for sync-OFF.
- `frontend/src/App.svelte` (or wherever auth gating lives): on boot, probe `/sync/status`; route to
  the right login mode; after login, pull + decrypt + hydrate before showing the app.
- Sidebar/`CreateSessionSheet`: **no change** — they read the hydrated localStorage list as today.
  "Add server" on any device now propagates because the mutation pushes to the hub.

## Data flow (sync ON, returning user)

```
PWA boot
  └─ GET /sync/status → {enabled:true, registered:true}
       └─ show "Entrar"
            └─ GET /sync/prelogin?user → {salt, iterations}
            └─ derive masterKey → authHash, encKey   (in browser)
            └─ POST /sync/login {user, authHash} → Set-Cookie cp_sync
            └─ GET /sync/vault → {enc_blob, rev}
            └─ decrypt(encKey, enc_blob) → serverList → write to localStorage
            └─ app renders with VPS + home + work already present
  add/remove/rename server
       └─ re-encrypt list → PUT /sync/vault {enc_blob, base_rev=rev}
            └─ 200 {rev+1}   |   409 → pull, merge, retry
```

## Security checklist (must hold)

- Password never transmitted; only `authHash` (a one-way HKDF branch) leaves the browser.
- `encKey` never transmitted; tokens are decryptable only on a client that knows the password.
- Hub at rest holds: salt, auth verifier, ciphertext. A full VPS compromise yields no usable token.
- Login rate-limited; session cookie HttpOnly + signed; `Secure` under TLS.
- Registration single-shot, bootstrap-gated.
- `prelogin` does not confirm username existence (no enumeration).
- Transport assumed HTTPS (VPS reverse proxy / Tailscale). Document that sync over plain HTTP is
  unsupported for production use.

## Open implementation details (defer to plan)

- Exact HKDF/PBKDF2 wrapping in WebCrypto (PBKDF2 → raw bits → HKDF via `deriveBits`).
- Cookie secret lifecycle (`CP_SYNC_SESSION_SECRET` persisted vs random-at-boot).
- Whether to clear the local list on logout (yes, default) vs keep for offline.

## Backend port (reference)

Backend binds `:8765` by default (`settings.port`, override `CP_PORT`); the PWA dev server / Caddy
serves the front on `:5173`. On the VPS the hub runs on the same `:8765`, fronted by TLS.
