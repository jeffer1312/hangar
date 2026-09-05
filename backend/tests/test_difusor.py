"""difusor.Difusor: uma fonte por chave, N ouvintes. E o que deixa desktop e celular abrirem o
mesmo chat sem cada SSE rodar o proprio monitor (2x has-session + capture-pane a cada 0,75s)."""
import asyncio

import pytest

from app.difusor import Difusor


class _Fonte:
    """Fabrica contavel: cada instancia e um async generator que solta `itens` com pausas."""

    def __init__(self, itens, pausa=0.01):
        self.itens = itens
        self.pausa = pausa
        self.criadas = 0
        self.canceladas = 0

    def __call__(self):
        self.criadas += 1
        return self._gerar()

    async def _gerar(self):
        try:
            for it in self.itens:
                yield it
                await asyncio.sleep(self.pausa)
        except asyncio.CancelledError:
            self.canceladas += 1
            raise


async def _colher(agen, n=None):
    out = []
    async for ev in agen:
        out.append(ev)
        if n is not None and len(out) >= n:
            break
    return out


async def test_dois_ouvintes_uma_fonte_e_os_dois_veem_tudo():
    d = Difusor()
    fonte = _Fonte(["a", "b", "c"])
    a, b = await asyncio.wait_for(
        asyncio.gather(_colher(d.ouvir("k", fonte)), _colher(d.ouvir("k", fonte))), timeout=5)
    assert fonte.criadas == 1
    assert a == ["a", "b", "c"] and b == ["a", "b", "c"]


async def test_quem_chega_depois_recebe_o_ultimo_evento_na_entrada():
    # StateMonitor so emite em MUDANCA: sem o retrato, o 2o SSE ficaria sem estado ate a proxima.
    d = Difusor()
    fonte = _Fonte(["idle", "working"], pausa=0.05)
    primeiro = asyncio.create_task(_colher(d.ouvir("k", fonte)))
    await asyncio.sleep(0.02)                      # "idle" ja saiu
    tarde = await asyncio.wait_for(_colher(d.ouvir("k", fonte)), timeout=5)
    await primeiro
    assert tarde == ["idle", "working"]


async def test_fonte_morre_com_o_ultimo_ouvinte_e_renasce_no_proximo():
    d = Difusor()
    fonte = _Fonte(["x"] * 100, pausa=0.01)
    t1 = asyncio.create_task(_colher(d.ouvir("k", fonte), n=2))
    t2 = asyncio.create_task(_colher(d.ouvir("k", fonte), n=2))
    await asyncio.gather(t1, t2)
    await asyncio.sleep(0.05)
    assert fonte.criadas == 1 and fonte.canceladas == 1
    await asyncio.wait_for(_colher(d.ouvir("k", fonte), n=1), timeout=5)
    assert fonte.criadas == 2


async def test_erro_da_fonte_chega_em_cada_ouvinte():
    # O pump do SSE trata excecao como `__error__` (derruba a conexao em vez de engolir).
    d = Difusor()

    async def quebra():
        yield "a"
        raise RuntimeError("boom")

    async def ouvir():
        with pytest.raises(RuntimeError, match="boom"):
            await _colher(d.ouvir("k", quebra))

    await asyncio.wait_for(asyncio.gather(ouvir(), ouvir()), timeout=5)


async def test_chaves_diferentes_nao_se_misturam():
    d = Difusor()
    f1, f2 = _Fonte(["um"]), _Fonte(["dois"])
    a, b = await asyncio.gather(_colher(d.ouvir("k1", f1)), _colher(d.ouvir("k2", f2)))
    assert a == ["um"] and b == ["dois"]
