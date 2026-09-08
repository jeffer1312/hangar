"""Uma fonte por chave, N ouvintes.

O SSE abre um monitor de estado por conexao; desktop e celular no mesmo chat eram dois monitores
(2x `has-session` + `capture-pane` a cada 0,75s). Aqui a fonte roda uma vez por chave, cada
ouvinte recebe o ultimo evento ao entrar (o monitor so emite em MUDANCA — sem o retrato, o 2o
ouvinte ficaria mudo ate a proxima) e copia de cada evento seguinte. A fonte morre com o ultimo
ouvinte e renasce no proximo. Excecao da fonte chega em cada ouvinte, pra quem consome tratar
como falha, nao como fim silencioso."""
import asyncio
from typing import AsyncIterator, Callable, Hashable

_FIM = object()


class _Erro:
    def __init__(self, exc: BaseException) -> None:
        self.exc = exc


class _Fonte:
    def __init__(self, agen: AsyncIterator) -> None:
        self.agen = agen
        self.ouvintes: list[asyncio.Queue] = []
        self.ultimo = None
        self.tarefa: asyncio.Task | None = None

    def espalhar(self, item) -> None:
        for fila in self.ouvintes:
            fila.put_nowait(item)


class Difusor:
    def __init__(self) -> None:
        self._fontes: dict[Hashable, _Fonte] = {}

    async def ouvir(self, chave: Hashable, fabrica: Callable[[], AsyncIterator]) -> AsyncIterator:
        fonte = self._fontes.get(chave)
        if fonte is None:
            fonte = _Fonte(fabrica())
            self._fontes[chave] = fonte
            fonte.tarefa = asyncio.create_task(self._bombear(chave, fonte))
        fila: asyncio.Queue = asyncio.Queue()
        fonte.ouvintes.append(fila)
        if fonte.ultimo is not None:
            fila.put_nowait(fonte.ultimo)
        try:
            while True:
                item = await fila.get()
                if item is _FIM:
                    return
                if isinstance(item, _Erro):
                    raise item.exc
                yield item
        finally:
            if fila in fonte.ouvintes:
                fonte.ouvintes.remove(fila)
            if not fonte.ouvintes and fonte.tarefa is not None and not fonte.tarefa.done():
                fonte.tarefa.cancel()
                if self._fontes.get(chave) is fonte:
                    del self._fontes[chave]

    async def _bombear(self, chave: Hashable, fonte: _Fonte) -> None:
        try:
            async for item in fonte.agen:
                fonte.ultimo = item
                fonte.espalhar(item)
            fonte.espalhar(_FIM)
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            fonte.espalhar(_Erro(exc))
        finally:
            if self._fontes.get(chave) is fonte:
                del self._fontes[chave]
