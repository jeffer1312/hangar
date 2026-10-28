"""Bindings C da autoridade Rust; nenhum estado do contador é calculado no Python."""
from __future__ import annotations

import asyncio
import ctypes
import json
import secrets
import tempfile
import threading
from pathlib import Path

from app import rust_bins

HTTP, BROWSER_WS, VOICE_WS, TERMINAL_WS, OWNER_PROBE = range(5)
OWNER, GUEST, UNAUTHORIZED, BLOCKED = range(4)
RECEIPT = "x-hangar-auth-receipt"
EMPTY_QUERY = "x-hangar-auth-empty-query"


class Unavailable(RuntimeError):
    def __init__(self, code="auth_store_unavailable"):
        super().__init__(code)
        self.code = code


class Decision(ctypes.Structure):
    _fields_ = [("status", ctypes.c_uint32), ("failure_recorded", ctypes.c_uint32),
                ("became_blocked", ctypes.c_uint32)]


def _checked(code):
    if code:
        raise Unavailable({1: "auth_store_unavailable", 2: "auth_store_invalid", 3: "auth_operation_failed",
                           4: "auth_abi_mismatch", 5: "auth_epoch_mismatch"}.get(code, "auth_abi_mismatch"))


def _json(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=True).encode("utf-8")


def _decision(result):
    if result.status not in (OWNER, GUEST, UNAUTHORIZED, BLOCKED) or result.failure_recorded not in (0, 1) or result.became_blocked not in (0, 1):
        raise Unavailable("auth_abi_mismatch")
    return result


class Context:
    def __init__(self, binary: Path, token: str, upstream_port: int):
        library, self.library_hash = rust_bins.auth_pair(binary)
        self.library = ctypes.CDLL(str(library))
        abi = self.library.hangar_auth_abi_v1
        abi.argtypes, abi.restype = [], ctypes.c_uint32
        if abi() != 1:
            raise Unavailable("auth_abi_mismatch")
        self.library.hangar_auth_open_v1.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_void_p)]
        self.library.hangar_auth_open_v1.restype = ctypes.c_uint32
        self.library.hangar_auth_evaluate_v1.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(Decision)]
        self.library.hangar_auth_evaluate_v1.restype = ctypes.c_uint32
        self.library.hangar_auth_verify_receipt_v1.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t,
                                                            ctypes.c_char_p, ctypes.c_size_t, ctypes.POINTER(Decision)]
        self.library.hangar_auth_verify_receipt_v1.restype = ctypes.c_uint32
        self.library.hangar_auth_close_v1.argtypes = [ctypes.c_void_p]
        self.library.hangar_auth_close_v1.restype = None
        self.directory = tempfile.TemporaryDirectory(prefix="hangar-auth-")
        self.upstream_port = upstream_port
        self.config = {"abi": 1, "store_version": 1, "mode": "create",
                       "state_path": str(Path(self.directory.name).resolve() / "state.json"),
                       "parent_epoch": secrets.token_hex(16), "expected_token": token,
                       "receipt_key": secrets.token_hex(32)}
        self.handle = ctypes.c_void_p()
        self._calls = threading.RLock()
        data = _json(self.config)
        _checked(self.library.hangar_auth_open_v1(data, len(data), ctypes.byref(self.handle)))
        self.config["mode"] = "attach"

    def child_env(self, binary: Path):
        _, digest = rust_bins.auth_pair(binary)
        if digest != self.library_hash:
            raise Unavailable("auth_pair_changed_restart_parent")
        return {"HANGAR_AUTH_CONTEXT": _json(self.config).decode("ascii")}

    def evaluate(self, profile, origin, token, *, verified_guest=False, login_guest=False):
        data = _json({"profile": profile, "origin": origin,
                      "presented_token": list(token.encode("utf-8")) if token is not None else None,
                      "verified_guest": verified_guest, "login_guest": login_guest})
        result = Decision()
        self._call(self.library.hangar_auth_evaluate_v1, data, len(data), ctypes.byref(result))
        return _decision(result)

    def request(self, request, token, source):
        scope = request.scope
        server = scope.get("server")
        receipt = request.headers.get(RECEIPT)
        if server and server[1] == self.upstream_port and receipt is not None:
            raw = scope.get("raw_path", scope["path"].encode("utf-8"))
            query = scope.get("query_string", b"")
            empty_query = request.headers.get(EMPTY_QUERY)
            if empty_query not in (None, "1") or (query and empty_query is not None):
                raise Unavailable("auth_receipt_invalid")
            try:
                raw_target = (raw + (b"?" + query if query or empty_query == "1" else b"")).decode("ascii")
            except UnicodeError:
                raise Unavailable("auth_request_invalid") from None
            context = {"connection_kind": "private_upstream", "profile": OWNER_PROBE,
                       "origin": request.client.host if request.client else "?",
                       "method": getattr(request, "method", "GET"), "raw_target": raw_target,
                       "credential_source": source,
                       "presented_token": list(token.encode("utf-8")) if token is not None else None}
            data, encoded = _json(context), receipt.encode("utf-8")
            result = Decision()
            self._call(self.library.hangar_auth_verify_receipt_v1, data, len(data), encoded, len(encoded), ctypes.byref(result))
            _decision(result)
            if result.status == OWNER:
                return result
            raise Unavailable("auth_receipt_invalid")
        return self.evaluate(HTTP, request.client.host if request.client else "?", token)

    def close(self):
        with self._calls:
            if self.handle.value:
                self.library.hangar_auth_close_v1(self.handle)
                self.handle = ctypes.c_void_p()
                self.directory.cleanup()

    def _call(self, function, *arguments):
        # O handle não pode ser solto enquanto uma thread cancelada ainda executa a FFI.
        with self._calls:
            if not self.handle.value:
                raise Unavailable()
            _checked(function(self.handle, *arguments))


# Uma referência fixada por vida do pai: queda do filho ou fallback não a desfazem.
_context: Context | None = None


def current():
    return _context


def activate(binary, token, upstream_port):
    global _context
    if _context is None:
        _context = Context(binary, token, upstream_port)
    elif _context.config["expected_token"] != token or not _context.handle.value:
        raise Unavailable("auth_context_changed")
    elif _context.upstream_port == 0:
        _context.upstream_port = upstream_port
    elif _context.upstream_port != upstream_port:
        raise Unavailable("auth_upstream_changed")
    return _context


def shutdown():
    # Chamado depois do filho e do executor de threads do loop encerrarem.
    if _context is not None:
        _context.close()


async def websocket(ws, profile, *, verified_guest=False, login_guest=False):
    context = current()
    if context is None:
        return None
    try:
        origin = ws.client.host if ws.client else ""
        result = await asyncio.to_thread(context.evaluate, profile, origin, ws.query_params.get("token", ""),
                                         verified_guest=verified_guest, login_guest=login_guest)
        if result.became_blocked:
            from app.auth import _log, _MAX_FAILS, _WINDOW
            await asyncio.to_thread(_log.warning, "[auth] %s errou o token %d vezes em %.0fs — bloqueado por %.0fs",
                                    origin, _MAX_FAILS, _WINDOW, _WINDOW)
        return result
    except Unavailable as error:
        if ws.application_state.value == 1:
            await ws.close(code=1013, reason=error.code)
        else:
            from starlette.responses import JSONResponse
            await ws.send_denial_response(JSONResponse({"detail": {"error_code": error.code}}, status_code=503))
        return False
