import asyncio
from typing import AsyncIterator

# Espelha PreviewBroker (backend/app/preview.py): mesma interface publica (get/subscribe,
# version + Condition + ref-count) e mesma semantica de coalescimento (full-replace, subscriber
# lento perde frames intermediarios). A DIFERENCA: Codex nao tem pane de tmux pra fazer poll --
# o texto em voo chega por PUSH (deltas do app-server, via CodexAdapter.state_monitor). Por isso
# nao ha _loop/_task; so um setter publico (push) que atualiza o slot e acorda os subscribers.


class CodexPreviewSource:
    """UMA instancia por sessao (por nome), igual ao PreviewBroker. Sem _loop: o texto chega via
    push(), nao poll. Ref-count: instancia sai do registry quando o ultimo subscriber sai."""

    _sources: dict[str, "CodexPreviewSource"] = {}

    def __init__(self, name: str):
        self.name = name
        self.text = ""
        self.version = 0
        self._cond = asyncio.Condition()
        self._subs = 0

    @classmethod
    def get(cls, name: str) -> "CodexPreviewSource":
        s = cls._sources.get(name)
        if s is None:
            s = cls(name)
            cls._sources[name] = s
        return s

    async def push(self, text: str) -> None:
        """Atualiza o texto em voo (chamado pelo state_monitor a cada delta acumulado, ou com ""
        pra limpar no fim do turno). Diff-gate igual ao _loop do PreviewBroker: so notifica em
        mudanca, evita spam de version pra texto repetido."""
        if text == self.text:
            return
        async with self._cond:
            self.text = text
            self.version += 1
            self._cond.notify_all()

    async def subscribe(self) -> AsyncIterator[str]:
        """Emite o texto mais recente (full-replace) a cada mudanca. Mesma mecanica do
        PreviewBroker.subscribe (coalescido por version + slot unico)."""
        async with self._cond:
            self._subs += 1
        last = -1
        try:
            while True:
                async with self._cond:
                    await self._cond.wait_for(lambda: self.version != last)
                    last = self.version
                    text = self.text
                yield text
        finally:
            # Limpeza sincrona (sem await entre as linhas), mesmo motivo do PreviewBroker: um
            # CancelledError no acquire do lock nao pode deixar _subs sem decrementar.
            self._subs -= 1
            if self._subs <= 0:
                self._sources.pop(self.name, None)
