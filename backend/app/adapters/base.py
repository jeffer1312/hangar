"""Contrato `Adapter`: abstrai as capacidades acopladas ao provider (Claude/Codex/...) — ler
transcript -> ChatEvent, classificar estado -> StateEvent, e mandar/checar input. `ChatEvent` e
`StateEvent` sao o contrato IMUTAVEL com o front (definidos em app.models, reexportados por
app.transcript/app.state); o Adapter troca so a FONTE de cada evento, nunca o shape."""
from typing import AsyncIterator, Callable, Protocol

from app.transcript import ChatEvent
from app.state import StateEvent


class Adapter(Protocol):
    provider: str

    def transcript_stream(self, path: str) -> AsyncIterator[ChatEvent]: ...

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]: ...

    async def drain(self, name: str, path: str) -> int: ...

    async def send_prompt(self, name: str, text: str) -> str: ...  # "sent" | "deferred"

    async def deliverable(self, name: str) -> bool: ...

    def spawn_command(self, cwd: str, session_id: str) -> list[str]: ...

    def transcript_path(self, cwd: str, session_id: str) -> str: ...

    # Opcionais (feature-detect via hasattr no caller; Codex nao implementa no v1):
    #   answer_questions, set_model_effort, transcript_image
