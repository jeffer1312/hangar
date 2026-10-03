"""ClaudeAdapter: casca fina que amarra o `Adapter` Protocol aos modulos ja existentes do Claude
(transcript.py/state.py/terminal_input.py). ZERO logica nova de Claude — so delegacao; o
comportamento de hoje (tmux, --session-id, hooks) fica intocado."""
import asyncio
from pathlib import Path
from typing import AsyncIterator, Callable

from app.config import settings
from app.state import StateEvent, StateMonitor
from app.transcript import ChatEvent, TranscriptTailer
from app import model_args
from app import plugin_bridge
from app import terminal_input as ti

# Mesma regex de app.registry.sanitize_cwd. Duplicada (nao importada) pra nao criar ciclo
# adapters.claude -> registry -> adapters (registry importa get_adapter em create()).


class ClaudeAdapter:
    provider = "claude"

    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        return TranscriptTailer(path).follow(start_offset)

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        return StateMonitor(name, sid_get=sid_get, observe_permission=True, provider="claude").stream()

    async def drain(self, name: str, path: str) -> int:
        # ti.drain e sincrono (digita no tty via subprocess tmux) -> thread, como sse.py ja fazia
        # direto antes desta casca existir.
        return await asyncio.to_thread(ti.drain, name, path)

    async def send_prompt(self, name: str, text: str) -> str:
        return await asyncio.to_thread(ti.TerminalInput().send_prompt, name, text)

    async def deliverable(self, name: str) -> bool:
        return await asyncio.to_thread(ti.deliverable, name)

    def spawn_command(self, cwd: str, session_id: str,
                      model: str | None = None, effort: str | None = None,
                      permission_mode: str | None = None) -> list[str]:
        argv = ["claude", "--session-id", session_id]
        # O plugin também carrega da pasta de skills da conta, inclusive fora do Hangar; o
        # `--plugin-dir` só entra quando ele não está lá ou o CLI não liga mods por padrão.
        for raiz in plugin_bridge.raizes_dos_plugins():
            argv += ["--plugin-dir", raiz]
        return argv + model_args.args_de("claude", model, effort, permission_mode)

    def transcript_path(self, cwd: str, session_id: str) -> str:
        from app.registry import sanitize_cwd   # local: registry importa os adapters
        return str(Path(settings.projects_dir) / sanitize_cwd(cwd) / f"{session_id}.jsonl")
