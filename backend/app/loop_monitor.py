"""Atraso do event loop e espera no pool padrão, gravados no diário.

Quando o backend para de responder, o vigia mata o processo e o log simplesmente acaba: não sobra
nada dizendo se o loop estava bloqueado por uma chamada síncrona ou se o pool de `to_thread` estava
com a fila cheia. Este módulo deixa essa evidência antes da morte. A thread de vigia existe porque,
com o loop parado, nenhuma corrotina roda para contar o que aconteceu.
"""
import asyncio
import logging
import os
import sys
import threading
import time
import traceback
from collections import Counter

from app import diag

_log = logging.getLogger("hangar.loop_monitor")

TICK_S = 1.0
LOOP_LAG_WARN_S = 1.0
POOL_PROBE_EVERY_S = 5.0
POOL_WAIT_WARN_S = 2.0
LOOP_STALL_S = 5.0
_MIN_GAP_S = 10.0   # no máximo uma linha de cada tipo nesse intervalo; o pior valor vai junto

_APP_MARK = f"{os.sep}app{os.sep}"


def _frame_label(fs: traceback.FrameSummary) -> str:
    name = fs.filename.replace("\\", "/").rsplit("/", 1)[-1]
    return f"{name}:{fs.lineno} {fs.name}"


def compact_stack(frame, limit: int = 6) -> str:
    """Molduras de dentro para fora, preferindo as do Hangar. Só código, nunca valores."""
    frames = traceback.extract_stack(frame)
    ours = [f for f in frames if _APP_MARK in f.filename]
    chosen = list(reversed(ours))[:limit - 1]
    innermost = frames[-1] if frames else None
    if innermost is not None and (not chosen or chosen[0] is not innermost):
        chosen.insert(0, innermost)
    return " < ".join(_frame_label(f) for f in chosen[:limit])


def _busy_pool_threads() -> list[str]:
    """Onde cada thread do pool padrão do asyncio está agora (as ociosas ficam no `queue.get`)."""
    frames = sys._current_frames()
    labels = []
    for t in threading.enumerate():
        if not t.name.startswith("asyncio_"):
            continue
        frame = frames.get(t.ident)
        if frame is None:
            continue
        # Ociosa fica no `_worker` esperando a fila; ocupada está dentro de `_WorkItem.run`.
        if not any(f.name == "run" and f.filename.endswith("thread.py")
                   for f in traceback.extract_stack(frame)):
            continue
        labels.append(compact_stack(frame, limit=2))
    return labels


def _queue_size(loop: asyncio.AbstractEventLoop) -> int:
    executor = getattr(loop, "_default_executor", None)
    queue = getattr(executor, "_work_queue", None)
    try:
        return queue.qsize() if queue is not None else -1
    except Exception:
        return -1


class _Throttle:
    def __init__(self) -> None:
        self.last = 0.0
        self.worst = 0

    def offer(self, value_ms: int) -> int | None:
        """Devolve o pior valor acumulado quando pode gravar; senão guarda e devolve None."""
        self.worst = max(self.worst, value_ms)
        now = time.monotonic()
        if now - self.last < _MIN_GAP_S:
            return None
        self.last, worst, self.worst = now, self.worst, 0
        return worst


class _Monitor:
    def __init__(self) -> None:
        self.heartbeat = time.monotonic()
        self.loop_thread: int | None = None
        self.stop = threading.Event()
        self.lag = _Throttle()
        self.pool = _Throttle()
        self.probe_running = False

    def watch_thread(self) -> None:
        reported = False
        while not self.stop.wait(1.0):
            stale = time.monotonic() - self.heartbeat
            if stale < LOOP_STALL_S:
                reported = False
                continue
            if reported or self.loop_thread is None:
                continue
            reported = True
            frame = sys._current_frames().get(self.loop_thread)
            stack = compact_stack(frame, limit=8) if frame is not None else "(sem moldura)"
            _log.warning("event loop parado ha %.1fs em: %s", stale, stack)
            diag.registrar("loop.travado", "erro", ms=int(stale * 1000), pilha=stack,
                           **diag.recursos())

    async def probe_pool(self, loop: asyncio.AbstractEventLoop) -> None:
        self.probe_running = True
        try:
            sent = time.monotonic()
            probe = loop.run_in_executor(None, time.monotonic)
            done, _ = await asyncio.wait({probe}, timeout=POOL_WAIT_WARN_S)
            if done:
                return
            # A foto sai enquanto a sonda ainda está na fila: depois dela, a fila já andou.
            queued = _queue_size(loop)
            busy = _busy_pool_threads()
            wait = await probe - sent
            worst = self.pool.offer(int(wait * 1000))
            if worst is None:
                return
            top = Counter(busy).most_common(3)
            summary = "; ".join(f"{n}x {label}" for label, n in top)
            _log.warning("pool padrao esperou %.1fs (fila=%d, ocupadas=%d): %s",
                         wait, queued, len(busy), summary)
            diag.registrar("pool.espera", "aviso", espera_ms=worst, quantidade=queued,
                           detalhe=summary or None, threads_backend=threading.active_count())
        except Exception:
            _log.debug("sonda do pool falhou", exc_info=True)
        finally:
            self.probe_running = False

    async def run(self) -> None:
        loop = asyncio.get_running_loop()
        self.loop_thread = threading.get_ident()
        self.heartbeat = time.monotonic()
        watcher = threading.Thread(target=self.watch_thread, name="hangar-loop-monitor", daemon=True)
        watcher.start()
        next_probe = time.monotonic()
        probes: set[asyncio.Task] = set()
        try:
            while True:
                before = loop.time()
                await asyncio.sleep(TICK_S)
                lag = loop.time() - before - TICK_S
                self.heartbeat = time.monotonic()
                if lag >= LOOP_LAG_WARN_S:
                    worst = self.lag.offer(int(lag * 1000))
                    if worst is not None:
                        diag.registrar("loop.atraso", "aviso", ms=worst)
                if self.heartbeat >= next_probe and not self.probe_running:
                    next_probe = self.heartbeat + POOL_PROBE_EVERY_S
                    task = asyncio.create_task(self.probe_pool(loop))
                    probes.add(task)
                    task.add_done_callback(probes.discard)
        finally:
            self.stop.set()
            for task in probes:
                task.cancel()


async def watch() -> None:
    await _Monitor().run()
