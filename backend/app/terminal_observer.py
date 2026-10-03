"""Observação terminal interna, com vínculo explícito e reserva no chamador."""
from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor
from contextvars import ContextVar
from contextlib import contextmanager
from dataclasses import dataclass, field
import ipaddress
import json
from http.client import HTTPException
import logging
import math
import sys
import threading
import time
import urllib.request
from uuid import uuid4

from app import diag, tmux

_log = logging.getLogger("hangar.terminal_observer")
TIMEOUT = 0.25
MAX_FAILURES = 3
MAX_BACKOFF = 30.0
_io_pool = ThreadPoolExecutor(max_workers=4, thread_name_prefix="hangar-terminal")
_io_slots = threading.BoundedSemaphore(4)
MAX_BODY = 16 * 1024 * 1024
HEARTBEAT = 20.0
_config: tuple[str, str] | None = None
_generation = 0
_epochs: dict[str, int] = {}
_bindings: dict[str, tuple[str, str]] = {}
_analysis: dict[str, tuple[tuple, float, str, dict]] = {}


@dataclass
class _SessionState:
    consumers: set[str] = field(default_factory=set)
    failures: int = 0
    retry_at: float = 0.0
    backoff: float = 1.0
    fallback_since: float | None = None


_sessions: dict[str, _SessionState] = {}
_consumers: dict[str, str] = {}
_unowned = _SessionState()
_current: ContextVar[Lease | None] = ContextVar("terminal_observer", default=None)
_STATES = {"idle", "working", "awaiting_input", "dead"}


def configure(address: str | None, secret: str | None) -> None:
    global _config, _generation, _unowned
    _config = None
    _generation += 1
    _analysis.clear()
    for name, session in _sessions.items():
        _sessions[name] = _SessionState(consumers=session.consumers)
    _unowned = _SessionState()
    if address is not None and secret:
        # O Supervisor fornece um IP literal: não resolvemos nomes nem usamos proxies.
        if not isinstance(address, str) or len(address) > 128:
            raise ValueError("invalid terminal address")
        host, port = address.rsplit(":", 1)
        ip = ipaddress.ip_address(host.strip("[]"))
        if not ip.is_loopback or not 0 < int(port) <= 65535:
            raise ValueError("terminal bridge requires loopback")
        host = f"[{ip}]" if ip.version == 6 else str(ip)
        _config = (f"{host}:{int(port)}", secret)


def forget(name: str) -> None:
    _epochs[name] = _epochs.get(name, 0) + 1
    _analysis.pop(name, None)


def _session(name: str) -> _SessionState:
    return _sessions.get(name, _unowned)


def _failure(name: str, code: str, started: float | None = None) -> None:
    session = _session(name)
    now = time.monotonic()
    pause_ms = 0
    if now >= session.retry_at:
        session.failures += 1
        if session.failures >= MAX_FAILURES:
            pause_ms = int(session.backoff * 1000)
            session.retry_at = now + session.backoff
            session.backoff = min(session.backoff * 2, MAX_BACKOFF)
    if session.fallback_since is None:
        session.fallback_since = now
        diag.registrar("terminal_observer.fallback", "aviso", sessao=name, codigo=code,
                       ms=max(0, int((now - started) * 1000)) if started is not None else 0)
        _log.warning("observação terminal de %s usa reserva Python: %s", name, code)
    if pause_ms:
        diag.registrar("terminal_observer.paused", "aviso", sessao=name, codigo=code,
                       limite_ms=pause_ms)


def _success(name: str) -> None:
    session = _session(name)
    session.failures, session.retry_at, session.backoff = 0, 0.0, 1.0
    if session.fallback_since is not None:
        diag.registrar("terminal_observer.recovered", "ok", sessao=name, codigo="rust_available",
                       ms=max(0, int((time.monotonic() - session.fallback_since) * 1000)))
        session.fallback_since = None


def _available(name: str = "") -> bool:
    return (_config is not None and sys.platform != "win32"
            and time.monotonic() >= _session(name).retry_at)


def _owner(payload: dict) -> tuple[str, str | None]:
    name = payload.get("name") or _consumers.get(payload.get("consumer"))
    if name in _sessions:
        return name, payload.get("consumer")
    source = _current.get()
    if source is not None and source.consumer in _consumers:
        return source.name, source.consumer
    return "", None


def _attempt(name: str, consumer: str | None) -> tuple:
    return name, consumer, _generation, _session(name)


def _belongs(attempt: tuple) -> bool:
    name, consumer, generation, session = attempt
    return (generation == _generation and session is _session(name)
            and (not name or (_consumers.get(consumer) == name and consumer in session.consumers)))


class _IoBusy(Exception):
    pass


async def _io(fn, *args):
    if not _io_slots.acquire(blocking=False):
        raise _IoBusy()
    slots = _io_slots
    try:
        job = _io_pool.submit(fn, *args)
    except BaseException:
        slots.release()
        raise
    # Cancelar a espera não libera a vaga de um trabalho que ainda está executando.
    job.add_done_callback(lambda _: slots.release())
    return await asyncio.wait_for(asyncio.wrap_future(job), TIMEOUT)


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # O segredo interno só pertence ao endereço configurado pelo Supervisor.
        return None


# A ponte só usa HTTP de loopback; não carrega certificados nem handlers de outros protocolos.
_opener = urllib.request.OpenerDirector()
for _handler in (urllib.request.ProxyHandler({}), urllib.request.HTTPHandler(),
                 urllib.request.HTTPDefaultErrorHandler(), urllib.request.HTTPErrorProcessor(), _NoRedirect()):
    _opener.add_handler(_handler)


def _http(config: tuple[str, str], payload: dict) -> dict | None:
    req = urllib.request.Request(f"http://{config[0]}/__hangar_server/terminal",
        data=json.dumps(payload, allow_nan=False).encode("utf-8"),
        headers={"Content-Type": "application/json", "x-hangar-internal": config[1]}, method="POST")
    with _opener.open(req, timeout=TIMEOUT) as response:
        if response.status != 200:
            raise urllib.error.HTTPError(req.full_url, response.status, "terminal status", None, None)
        body = response.read(MAX_BODY + 1)
        if len(body) > MAX_BODY:
            return None
        result = json.loads(body.decode("utf-8"), parse_constant=lambda _: None)
        return result if isinstance(result, dict) else None


async def _request(payload: dict) -> dict | None:
    config = _config
    name, consumer = _owner(payload)
    attempt = _attempt(name, consumer)
    started = time.monotonic()
    if not _available(name) or not _belongs(attempt):
        return None
    try:
        result = await _io(_http, config, payload)
        if not _belongs(attempt):
            return None
        if result is None:
            _failure(name, "invalid_http_response", started)
        elif payload.get("op") in ("acquire", "release"):
            if result != {}:
                _failure(name, "invalid_http_response", started)
                return None
        return result
    except (OSError, ValueError, TimeoutError, HTTPException, _IoBusy) as exc:
        # Nem pane, segredo, URL ou mensagem de exceção entram no diário.
        if _belongs(attempt):
            if isinstance(exc, urllib.error.HTTPError):
                code = f"http_{exc.code}"
            elif isinstance(exc, TimeoutError) or (isinstance(exc, urllib.error.URLError)
                                                  and isinstance(exc.reason, TimeoutError)):
                code = "http_timeout"
            elif isinstance(exc, HTTPException):
                code = "http_protocol"
            elif isinstance(exc, _IoBusy):
                code = "io_busy"
            elif isinstance(exc, ValueError):
                code = "invalid_http_response"
            else:
                code = "http_connection"
            _failure(name, code, started)
        return None


def _strings(value) -> bool:
    return isinstance(value, list) and all(isinstance(v, str) for v in value)


def valid_analysis(value) -> bool:
    fields = {"state", "label", "question", "options", "spinner", "status_line", "overlay",
              "login", "limit_reset", "preview", "codex_menu"}
    if not isinstance(value, dict) or set(value) != fields or not isinstance(value["state"], str) or value["state"] not in _STATES:
        return False
    if any(value[k] is not None and not isinstance(value[k], str)
           for k in ("label", "question", "spinner", "status_line", "limit_reset")):
        return False
    if value["options"] is not None and not _strings(value["options"]):
        return False
    menu = value["codex_menu"]
    return (type(value["overlay"]) is bool and type(value["login"]) is bool
        and isinstance(value["preview"], str)
        and (menu is None or (isinstance(menu, dict) and set(menu) == {"question", "options"}
            and (menu["question"] is None or isinstance(menu["question"], str)) and _strings(menu["options"]))))


class Lease:
    def __init__(self, name, provider, binding_get):
        self.name, self.provider, self.binding_get = name, provider, binding_get
        self.consumer = uuid4().hex
        self.binding = None
        self.open = False
        self._closed = False
        self.remote_generation = None

    def identity(self):
        if not self.open or self.provider not in ("claude", "codex"):
            return None
        attempt = _attempt(self.name, self.consumer)
        try:
            binding = self.binding_get()
        except Exception as exc:
            if _belongs(attempt):
                _failure(self.name, f"terminal_binding_{type(exc).__name__}")
            return None
        if not _belongs(attempt):
            return None
        if not isinstance(binding, str) or not binding:
            return None
        if binding != self.binding:
            self.binding = binding
            _bindings[self.name] = (self.provider, binding)
            forget(self.name)
        if _bindings.get(self.name) != (self.provider, binding):
            return None
        return self.provider, binding, _epochs.get(self.name, 0), _generation

    async def __aenter__(self):
        try:
            await self.start()
        except BaseException:
            await self.close()
            raise
        self.token = _current.set(self)
        return self

    async def start(self):
        if self._closed:
            return
        if self.consumer not in _consumers:
            _consumers[self.consumer] = self.name
            _sessions.setdefault(self.name, _SessionState()).consumers.add(self.consumer)
        attempt = _attempt(self.name, self.consumer)
        self.open = True
        try:
            self.binding = self.binding_get()
            if self.provider in ("claude", "codex") and isinstance(self.binding, str) and self.binding:
                if _bindings.get(self.name) != (self.provider, self.binding):
                    _bindings[self.name] = (self.provider, self.binding)
                    forget(self.name)
            await self.acquire()
        except Exception as exc:
            if not self.open or not _belongs(attempt):
                return
            _failure(self.name, f"lease_start_{type(exc).__name__}")
            try:
                # Libere só a referência remota parcial: o produtor tentará de novo.
                await self._release_remote()
            except Exception as close_exc:
                if self.open and _belongs(attempt):
                    _failure(self.name, f"lease_release_{type(close_exc).__name__}")

    async def __aexit__(self, *exc):
        _current.reset(self.token)
        await self.close()

    async def _release_remote(self):
        remote_generation, self.remote_generation = self.remote_generation, None
        if remote_generation == _generation:
            await _request({"op": "release", "consumer": self.consumer})

    async def close(self):
        self.open = False
        self._closed = True
        attempt = _attempt(self.name, self.consumer)
        try:
            await self._release_remote()
        except Exception as exc:
            if _belongs(attempt):
                _failure(self.name, f"lease_close_{type(exc).__name__}")
        finally:
            session = _sessions.get(self.name)
            if (session is not None and _consumers.get(self.consumer) == self.name
                    and self.consumer in session.consumers):
                _consumers.pop(self.consumer, None)
                session.consumers.discard(self.consumer)
                if not session.consumers:
                    _sessions.pop(self.name, None)
                    _analysis.pop(self.name, None)
                    _bindings.pop(self.name, None)
                    _epochs.pop(self.name, None)

    async def payload(self, op, started):
        identity = self.identity()
        if identity is None or not _available(self.name):
            return None
        attempt = _attempt(self.name, self.consumer)
        attempt_started = time.monotonic()
        try:
            target = await _io(tmux._pane_target, self.name)
        except Exception as exc:
            if self.open and _belongs(attempt):
                _failure(self.name, f"terminal_target_{type(exc).__name__}", attempt_started)
            return None
        if not self.open or not _belongs(attempt) or identity != self.identity():
            return None
        self.remote_generation = _generation
        return dict(op=op, consumer=self.consumer, name=self.name, provider=self.provider,
                    binding=identity[1], target=target, started=started, lines=200, colors=False, join=False)

    async def acquire(self):
        payload = await self.payload("acquire", time.monotonic())
        if payload is not None:
            await _request(payload)

    async def watch(self):
        while self.open:
            attempt = _attempt(self.name, self.consumer)
            try:
                await self.acquire()
            except Exception as exc:
                # A falha não pode encerrar a renovação nem registrar conteúdo privado.
                if self.open and _belongs(attempt):
                    _failure(self.name, f"lease_watch_{type(exc).__name__}")
            await asyncio.sleep(HEARTBEAT)


def lease(name, provider, binding_get) -> Lease:
    return Lease(name, provider, binding_get)


@contextmanager
def use(source):
    token = _current.set(source)
    try:
        yield
    finally:
        _current.reset(token)


def stamp(name: str) -> tuple:
    source = _current.get()
    identity = source.identity() if source is not None and source.name == name else None
    return (identity, _epochs.get(name, 0), _generation)


def retired(name: str) -> bool:
    source = _current.get()
    if source is None or not source.open or source.name != name or source.provider not in ("claude", "codex"):
        return False
    source.identity()
    return (isinstance(source.binding, str) and bool(source.binding)
            and _bindings.get(name) != (source.provider, source.binding))


async def capture(name: str, started: float) -> dict | None:
    source = _current.get()
    before = stamp(name)
    if source is None or source.name != name or type(started) not in (int, float) or not math.isfinite(started):
        return None
    attempt = _attempt(source.name, source.consumer)
    payload = await source.payload("capture", started)
    if payload is None:
        return None
    attempt_started = time.monotonic()
    result = await _request(payload)
    if not source.open or not _belongs(attempt) or before != stamp(name):
        return None
    if result is None:
        return None
    if not isinstance(result, dict) or set(result) != {"binding", "started", "text", "analysis"}:
        _failure(name, "invalid_frame", attempt_started)
        return None
    if (result["binding"] != payload["binding"] or type(result["started"]) not in (int, float)
            or result["started"] != started or not isinstance(result["text"], str) or not valid_analysis(result["analysis"])):
        _failure(name, "invalid_frame", attempt_started)
        return None
    _analysis[name] = (before, started, result["text"], result["analysis"])
    _success(name)
    return result


def frame_analysis(name: str, pane: str) -> dict | None:
    frame = _analysis.get(name)
    if frame is None or frame[0] != stamp(name) or frame[2] != pane:
        return None
    # A análise pertence também ao início do quadro no cache, nunca só ao texto.
    from app import state
    hit = state._frames.get(name)
    return frame[3] if hit is not None and hit == (frame[1], pane) else None
