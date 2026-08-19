"""KimiAdapter: casca fina que amarra o `Adapter` Protocol aos modulos do Kimi.

Espelha o PiAdapter de proposito: o Kimi tem transcript em arquivo (wire.jsonl) e roda numa TUI
dentro do tmux, entao NAO precisa do maquinario do Codex (app-server por sessao). Retomar e reabrir
o wire — com o detalhe de que o session-id NAO e escolhido por nos: quem liga pane->sessao e o
bilhete que o kimi_state_hook.py escreve (registry.kimi_session_file), nao o cmdline.

Por que nao `kimi acp` como canal: medido — um prompt injetado por outro processo nao entra no
contexto em memoria da TUI viva (app e pane divergem, o problema do app-server do Codex). O canal
de entrada que alcanca o processo vivo e tecla tmux, como no fallback do Pi.
"""
import asyncio
from typing import AsyncIterator, Callable

from app.adapters.kimi import sessions as kimi_sessions
from app.adapters.kimi.transcript import parse_line as kimi_parse_line
from app.state import StateEvent, StateMonitor
from app.transcript import ChatEvent, TranscriptTailer
from app import terminal_input as ti


class KimiAdapter:
    provider = "kimi"

    def transcript_stream(self, path: str, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        # parse_line solto (sem Stream com memoria): o parser do Kimi nao guarda estado entre
        # linhas — diferente do Pi, nao ha contexto de hook colado na mensagem do usuario pra
        # esperar a linha irma.
        return TranscriptTailer(path, parse_line=kimi_parse_line).follow(start_offset)

    def state_monitor(self, name: str, sid_get: Callable[[], str],
                      transcript_get: Callable[[], str | None] | None = None) -> AsyncIterator[StateEvent]:
        # hook_grace=None, mesma razao do Pi: o spinner do Kimi sao fases de lua (🌘🌗…), fora de
        # SPINNER_GLYPHS — a grace do Claude descartaria o marcador working NO MEIO do turno.
        # transcript_get: SO o Kimi passa. Um turno que comeca a partir de um prompt enfileirado na
        # TUI nao dispara evento nenhum, entao o marcador fica preso em "idle" o turno inteiro — e
        # sem spinner legivel o pane tambem nao salva. O wire.jsonl crescendo e a unica prova de
        # vida (ver state.corrige_ocioso_kimi).
        return StateMonitor(name, sid_get=sid_get, hook_grace=None,
                            transcript_get=transcript_get).stream()

    # provider="kimi" nos dois: o gate de "TUI pronta" (_wait_input_ready) casa marcas do RODAPE do
    # Claude por default, que o pane do Kimi nunca imprime -> sem o argumento, cada envio esperava
    # o timeout inteiro antes de digitar (o bug que o argumento provider existe pra matar no Pi).
    async def drain(self, name: str, path: str) -> int:
        return await asyncio.to_thread(ti.drain, name, path, "kimi")

    async def send_prompt(self, name: str, text: str) -> str:
        return await asyncio.to_thread(ti.TerminalInput().send_prompt, name, text, "kimi")

    async def deliverable(self, name: str) -> bool:
        return await asyncio.to_thread(ti.deliverable, name)

    def spawn_command(self, cwd: str, session_id: str,
                      model: str | None = None, effort: str | None = None) -> list[str]:
        # O session_id gerado pelo registry e IGNORADO de proposito: o Kimi nao aceita id escolhido
        # pelo caller (nao existe --session-id; -S so resume sessao existente). O sid real nasce no
        # 1o prompt e chega ao backend pelo bilhete do hook (registry.kimi_session_file).
        # model vira `--model <alias>` (alias `provider/id` do config.toml). Esforco nao tem flag
        # no CLI do Kimi — o model_args ja recusou o pedido antes de chegar aqui.
        from app import model_args
        return ["kimi"] + model_args.args_de("kimi", model, effort)

    def transcript_path(self, cwd: str, session_id: str) -> str:
        return kimi_sessions.transcript_path(cwd, session_id)
