"""PiAdapter: casca fina que amarra o `Adapter` Protocol aos modulos do Pi.

Espelha o ClaudeAdapter de proposito. O Pi tem transcript em arquivo e id escolhido pelo caller,
entao NAO precisa do maquinario do Codex (app-server WebSocket por sessao): retomar e reabrir o
JSONL, mesmo com o backend reiniciado e a TUI fechada.
"""
import asyncio
from typing import AsyncIterator, Callable

from app.adapters.pi import sessions as pi_sessions
from app.adapters.pi.transcript import parse_line
from app.state import StateEvent, StateMonitor
from app.transcript import ChatEvent, TranscriptTailer
from app import terminal_input as ti


class PiAdapter:
    provider = "pi"

    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        # kwarg e `parse_line=` (transcript.py:313), nao `parse=`. Mesma forma do
        # adapters/codex/adapter.py:516.
        return TranscriptTailer(path, parse_line=parse_line).follow(start_offset)

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        # Mesmo StateMonitor: a ancora de hook (HookState) ja cobre working/idle pelo marcador que
        # a extensao cp-state.ts escreve. O fallback de raspar o pane continua ligado como rede,
        # mas nao e o caminho normal — o Pi nao tem prompt de permissao pra detectar.
        # hook_grace=None: o loader do Pi e braille (⠋⠙⠹...), fora de SPINNER_GLYPHS, entao o
        # contador de polls-sem-spinner sobe durante o turno e a grace do Claude descartava o
        # marcador working NO MEIO da conversa (chat mostrando "ocioso" com o agente trabalhando).
        # A lista ja trata o marcador assim (registry.py:719, sem grace).
        return StateMonitor(name, sid_get=sid_get, hook_grace=None).stream()

    async def drain(self, name: str, path: str) -> int:
        return await asyncio.to_thread(ti.drain, name, path)

    async def send_prompt(self, name: str, text: str) -> str:
        return await asyncio.to_thread(ti.TerminalInput().send_prompt, name, text)

    async def deliverable(self, name: str) -> bool:
        return await asyncio.to_thread(ti.deliverable, name)

    def spawn_command(self, cwd: str, session_id: str) -> list[str]:
        return ["pi", "--session-id", session_id]

    def transcript_path(self, cwd: str, session_id: str) -> str:
        return pi_sessions.transcript_path(cwd, session_id)
