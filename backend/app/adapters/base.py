"""Contrato `Adapter`: abstrai as capacidades acopladas ao provider (Claude/Codex/...) — ler
transcript -> ChatEvent, classificar estado -> StateEvent, e mandar/checar input. `ChatEvent` e
`StateEvent` sao o contrato IMUTAVEL com o front (definidos em app.models, reexportados por
app.transcript/app.state); o Adapter troca so a FONTE de cada evento, nunca o shape."""
from typing import AsyncIterator, Callable, Protocol

from app.transcript import ChatEvent
from app.state import StateEvent


class Adapter(Protocol):
    provider: str

    # start_offset: retomada exata via Last-Event-ID do SSE (None = backfill do tail).
    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]: ...

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]: ...

    async def drain(self, name: str, path: str) -> int: ...

    # "partial" = digitou mas o Enter NAO submeteu (texto parcial no composer): quem chama nao pode
    # afirmar entrega nem gravar delivered. So o caminho de pane produz isso; o Codex (app-server)
    # nunca devolve.
    async def send_prompt(self, name: str, text: str) -> str: ...  # "sent" | "deferred" | "partial"

    async def deliverable(self, name: str) -> bool: ...

    def spawn_command(self, cwd: str, session_id: str) -> list[str]: ...

    def transcript_path(self, cwd: str, session_id: str) -> str: ...

    # Opcionais (feature-detect via hasattr no caller; Codex nao implementa no v1):
    #   answer_questions, set_model_effort, transcript_image
