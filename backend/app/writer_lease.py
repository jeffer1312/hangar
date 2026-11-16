"""Trava exclusiva do escritor de uma sessão, a mesma que o Rust toma (só biblioteca padrão)."""
from __future__ import annotations

import errno
import os
import sys
from pathlib import Path


class WriterLease:
    def __init__(self, path):
        path = path if isinstance(path, Path) else Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o600)
        self.file = os.fdopen(fd, "r+b", buffering=0)
        try:
            if sys.platform == "win32":
                import ctypes
                import msvcrt

                class Overlapped(ctypes.Structure):
                    _fields_ = [("internal", ctypes.c_size_t), ("internal_high", ctypes.c_size_t),
                                ("offset", ctypes.c_uint32), ("offset_high", ctypes.c_uint32),
                                ("event", ctypes.c_void_p)]

                lock = ctypes.WinDLL("kernel32", use_last_error=True).LockFileEx
                lock.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32,
                                 ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
                lock.restype = ctypes.c_int
                overlapped = Overlapped()
                if not lock(msvcrt.get_osfhandle(fd), 3, 0, 0xFFFFFFFF, 0xFFFFFFFF, ctypes.byref(overlapped)):
                    error = ctypes.get_last_error()
                    if error == 33:
                        raise BlockingIOError(errno.EAGAIN, "outro responsável possui a sessão")
                    raise ctypes.WinError(error)
            else:
                import fcntl
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BaseException:
            self.file.close()
            raise

    def close(self):
        self.file.close()

    @property
    def closed(self):
        return self.file.closed
