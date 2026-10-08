"""Existência de contas protegida por descritores compatíveis com o servidor Rust."""
from __future__ import annotations

import asyncio
from dataclasses import dataclass
from enum import Enum
import hashlib
import os
from pathlib import Path
import threading
import time


class Provider(str, Enum):
    CLAUDE = "claude"
    CODEX = "codex"


class GuardMode(str, Enum):
    SHARED = "shared"
    EXCLUSIVE = "exclusive"


def normalize_windows_path(value: str) -> str:
    value = value.replace("\\", "/")
    if value[:8].lower() == "//?/unc/":
        value = "//" + value[8:]
    elif value.startswith("//?/"):
        value = value[4:]
    # Por caractere, como o Rust: não aplicar casefold nem regras contextuais de Unicode.
    return "".join(char.lower() for char in value).rstrip("/")


def resolve_missing(path: Path) -> Path:
    try:
        return path.resolve(strict=True)
    except FileNotFoundError:
        if path.parent == path:
            raise
        parent = resolve_missing(path.parent)
        return parent.parent if path.name == ".." else parent / path.name


@dataclass(frozen=True)
class AccountKey:
    provider: Provider
    canonical_home: Path

    @classmethod
    def new(cls, provider: Provider | str, home: Path) -> AccountKey:
        canonical = str(resolve_missing(home.expanduser().absolute()))
        if os.name == "nt":
            canonical = normalize_windows_path(canonical)
        return cls(Provider(provider), Path(canonical))

    @property
    def digest(self) -> str:
        canonical = str(self.canonical_home)
        if os.name == "nt":
            canonical = normalize_windows_path(canonical)
        return key_digest(self.provider, canonical)


def key_digest(provider: Provider | str, canonical: str) -> str:
    return hashlib.sha256((Provider(provider).value + "\0" + canonical).encode("utf-8")).hexdigest()


def default_lock_root() -> Path:
    return Path.home() / ".hangar" / "account-locks"


class AccountLockError(RuntimeError):
    def __init__(self, code: str):
        self.code = code
        super().__init__(code)


class AccountGuard:
    def __init__(self, key: AccountKey, mode: GuardMode, file=None, *, state=None):
        self.key, self.mode = key, mode
        self._state = state or {"file": file, "references": 1, "lock": threading.Lock()}
        self._closed = False

    def retain(self) -> AccountGuard:
        """A thread retém o descritor antes de começar a operação bloqueante."""
        with self._state["lock"]:
            if self._closed:
                raise AccountLockError("account_guard_closed")
            self._state["references"] += 1
            return AccountGuard(self.key, self.mode, state=self._state)

    def close(self) -> None:
        with self._state["lock"]:
            if self._closed:
                return
            self._closed = True
            self._state["references"] -= 1
            if not self._state["references"]:
                self._state["file"].close()

    def __del__(self):
        self.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


def _try_lock(file, mode: GuardMode) -> bool:
    if os.name != "nt":
        import fcntl
        try:
            fcntl.flock(file.fileno(), fcntl.LOCK_NB | (fcntl.LOCK_SH if mode == GuardMode.SHARED else fcntl.LOCK_EX))
            return True
        except BlockingIOError:
            return False
    import ctypes
    from ctypes import wintypes
    import msvcrt

    class Overlapped(ctypes.Structure):
        _fields_ = [("Internal", ctypes.c_size_t), ("InternalHigh", ctypes.c_size_t),
                    ("Offset", wintypes.DWORD), ("OffsetHigh", wintypes.DWORD), ("hEvent", wintypes.HANDLE)]

    lock = ctypes.WinDLL("kernel32", use_last_error=True).LockFileEx
    lock.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD,
                     wintypes.DWORD, wintypes.DWORD, ctypes.POINTER(Overlapped)]
    lock.restype = wintypes.BOOL
    overlap = Overlapped()
    flags = 1 | (2 if mode == GuardMode.EXCLUSIVE else 0)
    if lock(msvcrt.get_osfhandle(file.fileno()), flags, 0, 0xFFFFFFFF, 0xFFFFFFFF, ctypes.byref(overlap)):
        return True
    error = ctypes.get_last_error()
    if error == 33:  # ERROR_LOCK_VIOLATION: único erro que autoriza esperar.
        return False
    raise ctypes.WinError(error)


def acquire(key: AccountKey, mode: GuardMode = GuardMode.SHARED, *, root: Path | None = None,
            deadline: float | None = None, cancel: threading.Event | None = None) -> AccountGuard:
    """Espera limitada fora do event loop; o prazo limita a aquisição, nunca a guarda."""
    root = root if root is not None else default_lock_root()
    root.mkdir(parents=True, exist_ok=True)
    file = (root / f"{key.digest}.lock").open("a+b")
    cancel = cancel if cancel is not None else threading.Event()
    deadline = time.monotonic() + 5 if deadline is None else deadline
    try:
        while True:
            if cancel.is_set():
                raise AccountLockError("account_lock_cancelled")
            if _try_lock(file, mode):
                return AccountGuard(key, mode, file)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AccountLockError("account_busy")
            cancel.wait(min(remaining, 0.025))
    except BaseException:
        file.close()
        raise


async def complete_on_cancel(awaitable):
    """O trabalho efetivo conserva suas guardas até terminar, mesmo sob cancelamentos repetidos."""
    worker = asyncio.ensure_future(awaitable)
    cancelled = False
    while not worker.done():
        try:
            await asyncio.shield(worker)
        except asyncio.CancelledError:
            cancelled = True
        except BaseException:
            break
    if cancelled:
        # Consumir a exceção evita um erro órfão sem perder o cancelamento do chamador.
        if not worker.cancelled():
            worker.exception()
        raise asyncio.CancelledError
    return worker.result()
