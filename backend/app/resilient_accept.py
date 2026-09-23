"""Accept do listener que sobrevive a cliente que desiste da conexão no Windows.

No ProactorEventLoop, um cliente que aborta entre o SYN e o fim do AcceptEx faz o accept levantar
OSError, e o `_start_serving` do asyncio trata qualquer OSError ali como fatal: fecha o listener e
o processo segue vivo sem ninguém escutar a porta. O erro é da conexão, não do listener — então os
códigos transitórios refazem o AcceptEx no mesmo listener, e o resto segue o caminho original.
"""
import asyncio
import logging
import sys

_log = logging.getLogger("hangar.accept")

# Erros de uma conexão que o par abortou antes do accept terminar — o listener segue são.
TRANSIENT_ERRORS = frozenset({
    64,      # ERROR_NETNAME_DELETED
    1236,    # ERROR_CONNECTION_ABORTED
    10053,   # WSAECONNABORTED
    10054,   # WSAECONNRESET
})

_PATCHED_MARK = "_hangar_resilient_accept"


def _is_transient(exc: BaseException) -> bool:
    return isinstance(exc, OSError) and getattr(exc, "winerror", None) in TRANSIENT_ERRORS


class _OuterFuture(asyncio.Future):
    # O `_stop_serving` cancela este future e fecha o socket na mesma pilha: o AcceptEx em
    # andamento tem de ser cancelado AQUI, antes do close, e não num callback do próximo ciclo.
    inner = None

    def cancel(self, msg=None):
        if self.inner is not None and not self.inner.done():
            self.inner.cancel()
        return super().cancel(msg=msg)


def wrap(original):
    """Devolve um `accept(self, listener)` que refaz o AcceptEx nos erros transitórios."""
    def accept(self, listener):
        outer = _OuterFuture(loop=self._loop)

        def attempt():
            try:
                outer.inner = original(self, listener)
            except OSError as exc:
                # Falha síncrona (listener já fechado etc.): o asyncio decide, como antes.
                if not outer.done():
                    outer.set_exception(exc)
                return
            outer.inner.add_done_callback(on_inner_done)

        def on_inner_done(inner):
            if outer.done():
                return
            if inner.cancelled():
                outer.cancel()
                return
            exc = inner.exception()
            if exc is None:
                outer.set_result(inner.result())
            elif _is_transient(exc) and listener.fileno() != -1:
                _log.warning("accept: conexão abortada pelo cliente (WinError %s); listener mantido",
                             exc.winerror)
                attempt()
            else:
                outer.set_exception(exc)

        attempt()
        return outer

    setattr(accept, _PATCHED_MARK, True)
    return accept


def install() -> None:
    """Aplica o embrulho no IocpProactor. Idempotente; fora do Windows não faz nada."""
    if sys.platform != "win32":
        return
    proactor = asyncio.windows_events.IocpProactor
    if getattr(proactor.accept, _PATCHED_MARK, False):
        return
    proactor.accept = wrap(proactor.accept)
