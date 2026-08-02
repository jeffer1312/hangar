"""PiAdapter: casca fina que amarra o `Adapter` Protocol aos modulos do Pi.

Espelha o ClaudeAdapter de proposito. O Pi tem transcript em arquivo e id escolhido pelo caller,
entao NAO precisa do maquinario do Codex (app-server WebSocket por sessao): retomar e reabrir o
JSONL, mesmo com o backend reiniciado e a TUI fechada.
"""
import asyncio
from typing import AsyncIterator, Callable

from app.adapters.pi import sessions as pi_sessions
from app.adapters.pi.transcript import Stream as PiStream
from app.state import StateEvent, StateMonitor
from app.transcript import ChatEvent, TranscriptTailer
from app import terminal_input as ti
from app import tmux


def _send_prompt_pi(name: str, text: str) -> str:
    """Roda em thread (mesma regra do resto do adapter): resolve o pane NA HORA, igual ao drain
    (terminal_input.py:544) -- sem pane_id, `send_prompt` nunca acha a linha do Pi (pi_inbox) e
    cai direto pra tecla, o bug que a Task 4 desta branch existe pra matar. Hoje este metodo e
    codigo morto (o unico `adapter.send_prompt` vivo e o do Codex, api.py:1235) -- mas se algo
    passar a rotear Pi por aqui, tem que sair correto, nao repetir o silencio."""
    pane_id = next((p["pane_id"] for p in tmux.list_panes_active() if p["name"] == name), None)
    return ti.TerminalInput().send_prompt(name, text, "pi", pane_id=pane_id)


class PiAdapter:
    provider = "pi"

    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        # kwarg e `parse_line=` (transcript.py:313), nao `parse=`. Mesma forma do
        # adapters/codex/adapter.py:516. Stream (e nao a funcao solta) porque o parser do Pi guarda
        # UMA linha de memoria pra tirar o contexto de hook da bolha do usuario — uma instancia por
        # stream, nunca compartilhada entre sessoes.
        return TranscriptTailer(path, parse_line=PiStream().parse_line).follow(start_offset)

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        # Mesmo StateMonitor: a ancora de hook (HookState) ja cobre working/idle pelo marcador que
        # a extensao cp-state.ts escreve. O fallback de raspar o pane continua ligado como rede,
        # mas nao e o caminho normal — o Pi nao tem prompt de permissao pra detectar.
        # hook_grace=None: o loader do Pi e braille (⠋⠙⠹...), fora de SPINNER_GLYPHS, entao o
        # contador de polls-sem-spinner sobe durante o turno e a grace do Claude descartava o
        # marcador working NO MEIO da conversa (chat mostrando "ocioso" com o agente trabalhando).
        # A lista ja trata o marcador assim (registry.py:719, sem grace).
        return StateMonitor(name, sid_get=sid_get, hook_grace=None).stream()

    # provider="pi" nos dois: o gate de "TUI pronta" (_wait_input_ready) casa marcas do RODAPE do
    # Claude, que o pane do Pi nunca imprime -> sem o argumento, cada envio esperava os 12s inteiros
    # de timeout (segurando o _send_lock) antes de digitar.
    async def drain(self, name: str, path: str) -> int:
        return await asyncio.to_thread(ti.drain, name, path, "pi")

    async def send_prompt(self, name: str, text: str) -> str:
        return await asyncio.to_thread(_send_prompt_pi, name, text)

    async def deliverable(self, name: str) -> bool:
        return await asyncio.to_thread(ti.deliverable, name)

    def spawn_command(self, cwd: str, session_id: str) -> list[str]:
        return ["pi", "--session-id", session_id]

    def transcript_path(self, cwd: str, session_id: str) -> str:
        return pi_sessions.transcript_path(cwd, session_id)
