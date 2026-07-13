"""ClaudeAdapter: casca fina que amarra o `Adapter` Protocol aos modulos ja existentes do Claude
(transcript.py/state.py/terminal_input.py). ZERO logica nova de Claude — so delegacao; o
comportamento de hoje (tmux, --session-id, hooks) fica intocado."""
import asyncio
import re
from pathlib import Path
from typing import AsyncIterator, Callable

from app.config import settings
from app.state import StateEvent, StateMonitor
from app.transcript import ChatEvent, TranscriptTailer
from app import terminal_input as ti

# Mesma regex de app.registry.sanitize_cwd. Duplicada (nao importada) pra nao criar ciclo
# adapters.claude -> registry -> adapters (registry importa get_adapter em create()).
_SANITIZE_RE = re.compile(r"[^A-Za-z0-9]")


class ClaudeAdapter:
    provider = "claude"

    def transcript_stream(self, path: str) -> AsyncIterator[ChatEvent]:
        return TranscriptTailer(path).follow()

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        return StateMonitor(name, sid_get=sid_get).stream()

    async def drain(self, name: str, path: str) -> int:
        # ti.drain e sincrono (digita no tty via subprocess tmux) -> thread, como sse.py ja fazia
        # direto antes desta casca existir.
        return await asyncio.to_thread(ti.drain, name, path)

    async def send_prompt(self, name: str, text: str) -> str:
        return await asyncio.to_thread(ti.TerminalInput().send_prompt, name, text)

    async def deliverable(self, name: str) -> bool:
        return await asyncio.to_thread(ti.deliverable, name)

    def spawn_command(self, cwd: str, session_id: str) -> list[str]:
        return ["claude", "--session-id", session_id]

    def transcript_path(self, cwd: str, session_id: str) -> str:
        return str(Path(settings.projects_dir) / _SANITIZE_RE.sub("-", cwd) / f"{session_id}.jsonl")
