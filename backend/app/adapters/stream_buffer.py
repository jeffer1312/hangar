# backend/app/adapters/stream_buffer.py
from __future__ import annotations

import asyncio
import io
import logging
import traceback
from collections.abc import Awaitable, Callable

_log = logging.getLogger(__name__)


def error_frames(error: BaseException) -> str:
    return "; ".join(f"{frame.filename}:{frame.lineno}" for frame in traceback.extract_tb(error.__traceback__))


class StreamBuffer:
    """Acumula pedaços e publica primeiro, periódico e último como texto completo."""

    def __init__(self, publish: Callable[[str], Awaitable[None]], *,
                 on_error: Callable[[Exception], None] | None = None,
                 interval: float = 0.15):
        self._publish = publish
        self._on_error = on_error
        self._interval = interval
        self._data = io.StringIO()
        self._generation = 0
        self._revision = 0
        self._published = False
        self._dirty = False
        self._last = 0.0
        self._pending: asyncio.Task | None = None
        self._tasks: set[asyncio.Task] = set()
        self._publishing = asyncio.Lock()

    @property
    def value(self) -> str:
        return self._data.getvalue()

    async def append(self, piece: str) -> None:
        if not piece:
            return
        self._data.write(piece)
        self._revision += 1
        self._dirty = True
        now = asyncio.get_running_loop().time()
        if not self._published or now - self._last >= self._interval:
            await self._emit(self._generation)
        else:
            self._schedule()

    def _schedule(self) -> None:
        if not self._dirty or (self._pending is not None and not self._pending.done()):
            return
        task = asyncio.create_task(self._later(self._generation))
        self._pending = task
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    async def _later(self, generation: int) -> None:
        failed = False
        try:
            delay = max(0.0, self._last + self._interval - asyncio.get_running_loop().time()) if self._published else 0.0
            await asyncio.sleep(delay)
            await self._emit(generation)
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            failed = True
            _log.warning("publicação parcial falhou: %s frames=%s", type(exc).__name__, error_frames(exc))
            if self._on_error is not None:
                try:
                    self._on_error(exc)
                except Exception as callback_error:
                    _log.warning("aviso da publicação parcial falhou: %s frames=%s",
                                 type(callback_error).__name__, error_frames(callback_error))
        finally:
            if self._pending is asyncio.current_task():
                self._pending = None
            if not failed and generation == self._generation and self._dirty:
                self._schedule()

    async def _emit(self, generation: int) -> None:
        async with self._publishing:
            if generation != self._generation or not self._dirty:
                return
            revision = self._revision
            await self._publish(self.value)
            if generation == self._generation:
                self._published = True
                self._last = asyncio.get_running_loop().time()
                self._dirty = revision != self._revision

    async def flush(self) -> None:
        await self._emit(self._generation)

    def rebind(self) -> None:
        self.invalidate(self.value)
        self._schedule()

    def invalidate(self, value: str = "") -> None:
        # Chamada só no event loop: uma geração encerrada nunca publica no bloco seguinte.
        self._generation += 1
        self._revision = 0
        self._data = io.StringIO(value)
        self._data.seek(0, io.SEEK_END)
        self._published = False
        self._dirty = bool(value)
        self._last = 0.0
        self._pending = None
        for task in tuple(self._tasks):
            task.cancel()

    async def discard(self) -> None:
        tasks = tuple(self._tasks)
        self.invalidate()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        async with self._publishing:
            pass

    async def reset(self, value: str = "") -> None:
        await self.discard()
        if value:
            self._data = io.StringIO(value)
            self._data.seek(0, io.SEEK_END)
            self._dirty = True
