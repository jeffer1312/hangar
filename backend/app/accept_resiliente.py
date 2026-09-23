"""Accept do listener que sobrevive a cliente que desiste da conexao no Windows.

No Windows o uvicorn roda no ProactorEventLoop (o backend precisa dele pros subprocessos). Ali,
quando o cliente aborta a conexao entre o SYN e o fim do AcceptEx, o `f.result()` do accept levanta
`OSError [WinError 64] The specified network name is no longer available`. O
`BaseProactorEventLoop._start_serving` trata QUALQUER OSError do accept como fatal: loga
"Accept failed on a socket" e FECHA O SOCKET DO LISTENER. O processo continua vivo, mas ninguem
mais escuta na porta — o app acusa "backend fora" ate o vigia reiniciar a tarefa.

Medido em 2026-09-23 09:17:57 (e antes em 2026-09-19 14:19:22): o app Electron abriu com o backend
lento, os pedidos estouraram timeout de 8-10s, um cliente sumiu bem no accept e a 8765 morreu.

O erro e da CONEXAO, nao do listener. Aqui o `IocpProactor.accept` e embrulhado: um desses erros
transitorios dispara um novo AcceptEx no mesmo listener em vez de chegar ao `_start_serving`.
Qualquer outro erro segue o caminho original do asyncio.
"""
import asyncio
import logging
import sys

_log = logging.getLogger("hangar.accept")

# Erros de uma conexao que o par abortou antes do accept terminar — o listener segue sao.
ERROS_TRANSITORIOS = frozenset({
    64,      # ERROR_NETNAME_DELETED
    1236,    # ERROR_CONNECTION_ABORTED
    10053,   # WSAECONNABORTED
    10054,   # WSAECONNRESET
})

_MARCA = "_hangar_accept_resiliente"


def _transitorio(exc: BaseException) -> bool:
    return isinstance(exc, OSError) and getattr(exc, "winerror", None) in ERROS_TRANSITORIOS


def embrulhar(original):
    """Devolve um `accept(self, listener)` que refaz o AcceptEx nos erros transitorios.

    O future devolvido e o que o `_start_serving` guarda em `_accept_futures` e cancela ao fechar
    o servidor: o cancelamento dele tem de chegar ao AcceptEx em andamento.
    """
    def accept(self, listener):
        externo = self._loop.create_future()
        atual = None

        def tentar():
            nonlocal atual
            try:
                atual = original(self, listener)
            except OSError as exc:
                # Falha sincrona (listener ja fechado, etc.): o asyncio decide, como antes.
                if not externo.done():
                    externo.set_exception(exc)
                return
            atual.add_done_callback(pronto)

        def pronto(interno):
            if externo.done():
                return
            if interno.cancelled():
                externo.cancel()
                return
            exc = interno.exception()
            if exc is None:
                externo.set_result(interno.result())
            elif _transitorio(exc) and listener.fileno() != -1:
                _log.warning("accept: conexao abortada pelo cliente (WinError %s); listener mantido",
                             exc.winerror)
                tentar()
            else:
                externo.set_exception(exc)

        def cancelado(fut):
            if fut.cancelled() and atual is not None and not atual.done():
                atual.cancel()

        externo.add_done_callback(cancelado)
        tentar()
        return externo

    setattr(accept, _MARCA, True)
    return accept


def instalar() -> None:
    """Aplica o embrulho no IocpProactor. Idempotente; fora do Windows nao faz nada."""
    if sys.platform != "win32":
        return
    proactor = asyncio.windows_events.IocpProactor
    if getattr(proactor.accept, _MARCA, False):
        return
    proactor.accept = embrulhar(proactor.accept)
