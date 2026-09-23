"""Accept que sobrevive a cliente abortando a conexão (WinError 64) no ProactorEventLoop."""
import asyncio
import sys

import pytest

from app import resilient_accept


class _WinError(OSError):
    """OSError com `winerror`, em qualquer plataforma (no Linux o construtor não preenche)."""
    def __init__(self, winerror):
        super().__init__(22, "The specified network name is no longer available")
        self.winerror = winerror


class _Listener:
    def __init__(self, fd=5):
        self.fd = fd

    def fileno(self):
        return self.fd


class _Proactor:
    def __init__(self, loop):
        self._loop = loop


def _fake_original(loop, results):
    """Fake do `IocpProactor.accept`: cada chamada devolve o próximo resultado da fila."""
    calls = []

    def original(self, listener):
        calls.append(listener)
        fut = loop.create_future()
        r = results.pop(0)
        if isinstance(r, BaseException):
            loop.call_soon(fut.set_exception, r)
        else:
            loop.call_soon(fut.set_result, r)
        return fut

    return original, calls


def test_transient_error_retries_and_delivers_the_connection():
    async def scenario():
        loop = asyncio.get_running_loop()
        original, calls = _fake_original(loop, [_WinError(64), _WinError(1236), ("conn", "addr")])
        fut = resilient_accept.wrap(original)(_Proactor(loop), _Listener())
        assert await fut == ("conn", "addr")
        assert len(calls) == 3

    asyncio.run(scenario())


def test_non_transient_error_reaches_asyncio():
    async def scenario():
        loop = asyncio.get_running_loop()
        original, calls = _fake_original(loop, [_WinError(5)])
        fut = resilient_accept.wrap(original)(_Proactor(loop), _Listener())
        with pytest.raises(OSError):
            await fut
        assert len(calls) == 1

    asyncio.run(scenario())


def test_closed_listener_does_not_retry():
    async def scenario():
        loop = asyncio.get_running_loop()
        original, calls = _fake_original(loop, [_WinError(64)])
        fut = resilient_accept.wrap(original)(_Proactor(loop), _Listener(fd=-1))
        with pytest.raises(OSError):
            await fut
        assert len(calls) == 1

    asyncio.run(scenario())


def test_cancel_reaches_the_pending_accept_synchronously():
    async def scenario():
        loop = asyncio.get_running_loop()
        inners = []

        def original(self, listener):
            inners.append(loop.create_future())   # nunca completa: accept pendente
            return inners[-1]

        fut = resilient_accept.wrap(original)(_Proactor(loop), _Listener())
        fut.cancel()
        # Sem ceder ao loop: o `_stop_serving` fecha o socket logo depois do cancel.
        assert inners[0].cancelled()
        assert fut.cancelled()

    asyncio.run(scenario())


def test_cancel_after_retry_reaches_the_new_accept():
    async def scenario():
        loop = asyncio.get_running_loop()
        inners = []

        def original(self, listener):
            inners.append(loop.create_future())
            if len(inners) == 1:
                loop.call_soon(inners[0].set_exception, _WinError(64))
            return inners[-1]

        fut = resilient_accept.wrap(original)(_Proactor(loop), _Listener())
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        assert len(inners) == 2
        fut.cancel()
        assert inners[1].cancelled()

    asyncio.run(scenario())


@pytest.mark.skipif(sys.platform != "win32", reason="IocpProactor só existe no Windows")
def test_proactor_server_keeps_listening_after_winerror_64(monkeypatch):
    """Ponta a ponta no loop real: sem o embrulho, o 1º accept falho fecharia o listener."""
    proactor = asyncio.windows_events.IocpProactor
    real = proactor.accept
    failures = [OSError(None, "The specified network name is no longer available", None, 64)]

    def accept_failing_once(self, listener):
        if failures:
            fut = self._loop.create_future()
            fut.set_exception(failures.pop())
            return fut
        return real(self, listener)

    monkeypatch.setattr(proactor, "accept", resilient_accept.wrap(accept_failing_once))

    async def scenario():
        async def echo(reader, writer):
            writer.write(await reader.read(4))
            await writer.drain()
            writer.close()

        server = await asyncio.start_server(echo, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        try:
            reader, writer = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", port), 5)
            writer.write(b"ping")
            assert await asyncio.wait_for(reader.read(4), 5) == b"ping"
            writer.close()
            assert not failures
            assert all(s.fileno() != -1 for s in server.sockets)
        finally:
            server.close()
            await server.wait_closed()

    asyncio.run(scenario())


def test_install_is_idempotent(monkeypatch):
    if sys.platform == "win32":
        # Restaura o accept original ao fim: o embrulho não vaza pros outros testes.
        proactor = asyncio.windows_events.IocpProactor
        monkeypatch.setattr(proactor, "accept", proactor.accept)
    resilient_accept.install()
    resilient_accept.install()
    if sys.platform == "win32":
        accept = asyncio.windows_events.IocpProactor.accept
        assert getattr(accept, resilient_accept._PATCHED_MARK)
