"""Contrato `Adapter`: abstrai as capacidades acopladas ao provider (Claude/Codex/...) de mandar e
checar input."""
from typing import Protocol


class Adapter(Protocol):
    provider: str

    async def drain(self, name: str, path: str) -> int: ...

    # "partial" = digitou mas o Enter NAO submeteu (texto parcial no composer): quem chama nao pode
    # afirmar entrega nem gravar delivered. So o caminho de pane produz isso; o Codex (app-server)
    # nunca devolve.
    async def send_prompt(self, name: str, text: str) -> str: ...  # "sent" | "deferred" | "partial"

    async def deliverable(self, name: str) -> bool: ...

    def spawn_command(self, cwd: str, session_id: str,
                      model: str | None = None, effort: str | None = None,
                      permission_mode: str | None = None) -> list[str]: ...

    def transcript_path(self, cwd: str, session_id: str) -> str: ...

    # Opcionais (feature-detect via hasattr no caller; Codex nao implementa no v1):
    #   answer_questions, set_model_effort, transcript_image
