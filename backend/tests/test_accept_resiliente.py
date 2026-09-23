"""Accept que sobrevive a cliente abortando a conexao (WinError 64) no ProactorEventLoop."""
import asyncio
import socket
import sys

import pytest

from app import accept_resiliente


class _ErroWin(OSError):
    """OSError com `winerror`, em qualquer plataforma (no Linux o construtor nao preenche)."""
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


def _original_com(loop, resultados):
    """Fake do `IocpProactor.accept`: cada chamada devolve o proximo resultado da fila."""
    chamadas = []

    def original(self, listener):
        chamadas.append(listener)
        fut = loop.create_future()
        r = resultados.pop(0)
        if isinstance(r, BaseException):
            loop.call_soon(fut.set_exception, r)
        else:
            loop.call_soon(fut.set_result, r)
        return fut

    return original, chamadas


def test_erro_transitorio_refaz_o_accept_e_entrega_a_conexao():
    async def cenario():
        loop = asyncio.get_running_loop()
        original, chamadas = _original_com(loop, [_ErroWin(64), _ErroWin(1236), ("conn", "addr")])
        fut = accept_resiliente.embrulhar(original)(_Proactor(loop), _Listener())
        assert await fut == ("conn", "addr")
        assert len(chamadas) == 3

    asyncio.run(cenario())


def test_erro_nao_transitorio_segue_para_o_asyncio():
    async def cenario():
        loop = asyncio.get_running_loop()
        original, chamadas = _original_com(loop, [_ErroWin(5)])
        fut = accept_resiliente.embrulhar(original)(_Proactor(loop), _Listener())
        with pytest.raises(OSError):
            await fut
        assert len(chamadas) == 1

    asyncio.run(cenario())


def test_listener_fechado_nao_refaz():
    async def cenario():
        loop = asyncio.get_running_loop()
        original, chamadas = _original_com(loop, [_ErroWin(64)])
        fut = accept_resiliente.embrulhar(original)(_Proactor(loop), _Listener(fd=-1))
        with pytest.raises(OSError):
            await fut
        assert len(chamadas) == 1

    asyncio.run(cenario())


def test_cancelar_o_externo_cancela_o_accept_em_andamento():
    async def cenario():
        loop = asyncio.get_running_loop()
        internos = []

        def original(self, listener):
            internos.append(loop.create_future())   # nunca completa: accept pendente
            return internos[-1]

        fut = accept_resiliente.embrulhar(original)(_Proactor(loop), _Listener())
        fut.cancel()
        await asyncio.sleep(0)
        assert internos[0].cancelled()

    asyncio.run(cenario())


@pytest.mark.skipif(sys.platform != "win32", reason="IocpProactor so existe no Windows")
def test_servidor_proactor_continua_escutando_apos_winerror_64(monkeypatch):
    """Ponta a ponta no loop real: sem o embrulho, o 1o accept falho fecharia o listener."""
    proactor = asyncio.windows_events.IocpProactor
    real = proactor.accept
    falhas = [OSError(None, "The specified network name is no longer available", None, 64)]

    def accept_que_falha_uma_vez(self, listener):
        if falhas:
            fut = self._loop.create_future()
            fut.set_exception(falhas.pop())
            return fut
        return real(self, listener)

    monkeypatch.setattr(proactor, "accept", accept_resiliente.embrulhar(accept_que_falha_uma_vez))

    async def cenario():
        async def eco(reader, writer):
            writer.write(await reader.read(4))
            await writer.drain()
            writer.close()

        servidor = await asyncio.start_server(eco, "127.0.0.1", 0)
        porta = servidor.sockets[0].getsockname()[1]
        try:
            reader, writer = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", porta), 5)
            writer.write(b"ping")
            assert await asyncio.wait_for(reader.read(4), 5) == b"ping"
            writer.close()
            assert not falhas
            assert all(s.fileno() != -1 for s in servidor.sockets)
        finally:
            servidor.close()
            await servidor.wait_closed()

    asyncio.run(cenario())


def test_instalar_e_idempotente(monkeypatch):
    if sys.platform == "win32":
        # Restaura o accept original ao fim: o embrulho nao vaza pros outros testes.
        proactor = asyncio.windows_events.IocpProactor
        monkeypatch.setattr(proactor, "accept", proactor.accept)
    accept_resiliente.instalar()
    accept_resiliente.instalar()
    if sys.platform == "win32":
        accept = asyncio.windows_events.IocpProactor.accept
        assert getattr(accept, accept_resiliente._MARCA)
