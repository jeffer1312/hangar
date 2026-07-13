"""CodexAdapter: junta o rollout (historico, via parse_rollout_line) com o app-server (JSON-RPC
ao vivo, via AppServerClient) por tras do `Adapter` Protocol. O coracao e map_state(), que
traduz uma notification do app-server (camelCase) num resultado NEUTRO e testavel; state_monitor
consome isso e emite o StateEvent real do app.

Nomes/shapes confirmados contra codex-cli 0.141.0 em docs/codex-app-server-contract.md.

Escopo desta task (ver brief): so map_state/transcript_stream/state_monitor/send_prompt/
deliverable + registro em PROVIDERS. `drain`/`spawn_command`/`transcript_path` do Adapter
Protocol e o lifecycle de attach() (nome tmux -> AppServerClient/threadId vivos) ficam pra Task 5
(registry/spawn) — CodexAdapter aqui e uma casca PARCIAL do Protocol de proposito."""
from dataclasses import dataclass
from typing import AsyncIterator, Callable, Optional

from app.adapters.codex.appserver import AppServerClient
from app.adapters.codex.rollout import parse_rollout_line
from app.state import StateEvent
from app.transcript import ChatEvent, TranscriptTailer


@dataclass
class MappedState:
    """Resultado CRU e testavel de map_state — NAO e o StateEvent do app (StateEvent exige
    `state` e nao tem campo de preview). state_monitor() traduz isto pro StateEvent real;
    preview_delta e um mecanismo PARALELO ainda sem fiacao no sse.py (ver nota em
    CodexAdapter._state_stream e o concern no report da task)."""
    state: Optional[str] = None          # "working" | "idle" | None (neutro/sem info)
    status_line: Optional[str] = None
    preview_delta: Optional[str] = None


def map_state(notif: dict) -> MappedState:
    """Mapeia UMA notification do app-server (`{"method": ..., "params": ...}`) -> MappedState.
    Method desconhecido (ou shape incompleto) -> MappedState() neutro, nunca levanta."""
    method = notif.get("method")
    params = notif.get("params") or {}

    if method == "turn/started":
        return MappedState(state="working")
    if method == "turn/completed":
        return MappedState(state="idle")

    if method == "thread/status/changed":
        status_type = (params.get("status") or {}).get("type")
        if status_type == "active":
            return MappedState(state="working")
        if status_type == "idle":
            return MappedState(state="idle")
        return MappedState()

    if method == "thread/tokenUsage/updated":
        usage = params.get("tokenUsage") or {}
        total = (usage.get("total") or {}).get("totalTokens")
        window = usage.get("modelContextWindow")
        if not total or not window:
            return MappedState()
        pct = round(total / window * 100)
        return MappedState(status_line=f"{pct}% context")

    if method == "item/agentMessage/delta":
        return MappedState(preview_delta=params.get("delta"))

    return MappedState()


class CodexAdapter:
    provider = "codex"

    def __init__(self) -> None:
        # nome da sessao tmux -> {"client": AppServerClient, "thread_id": str, "state": str,
        # "in_progress": bool}. Vazio ate attach() ser chamado (Task 5, ao spawnar o app-server
        # e dar thread/start) — sem entrada, a sessao e tratada como "dead"/"deliverable".
        self._sessions: dict[str, dict] = {}

    def attach(self, name: str, client: AppServerClient, thread_id: str) -> None:
        """Liga uma sessao tmux (por nome) a um AppServerClient + threadId ja vivos. Ponto de
        entrada unico que o lifecycle de spawn (Task 5) precisa chamar; nada aqui spawna nada."""
        self._sessions[name] = {"client": client, "thread_id": thread_id,
                                 "state": "idle", "in_progress": False}

    def transcript_stream(self, path: str) -> AsyncIterator[ChatEvent]:
        # Mesma mecanica de tail (backfill do tail + watch de append) do Claude, so trocando o
        # parser pro shape do rollout do Codex (snake_case, envelope {type, payload}).
        return TranscriptTailer(path, parse_line=parse_rollout_line).follow()

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        return self._state_stream(name)

    async def _state_stream(self, name: str) -> AsyncIterator[StateEvent]:
        sess = self._sessions.get(name)
        if sess is None:
            # Sem app-server anexado (nunca spawnado, ou attach() ainda nao rodou) -> "dead" pro
            # front, igual ao StateMonitor do Claude quando a sessao tmux some.
            yield StateEvent(session=name, state="dead")
            return
        client = sess["client"]
        # NOTA (concern, ver report): client.notifications() do AppServerClient real nunca
        # termina sozinho (fila interna infinita, ver appserver.py) — nao ha hoje um sinal de
        # "app-server morreu" exposto pro adapter detectar aqui. So a AUSENCIA de attach() (acima)
        # produz "dead"; morte MID-sessao fica sem cobertura nesta task.
        async for notif in client.notifications():
            mapped = map_state(notif)
            if mapped.state is not None:
                sess["state"] = mapped.state
                sess["in_progress"] = mapped.state == "working"
            if mapped.state is None and mapped.status_line is None:
                # Neutro (method desconhecido) ou so preview_delta: StateEvent nao tem campo de
                # preview -> nada a emitir aqui. O texto ao vivo do delta fica sem rota pro SSE
                # neste adapter (concern anotado no report da task).
                continue
            yield StateEvent(session=name, state=sess["state"], status_line=mapped.status_line)

    async def send_prompt(self, name: str, text: str) -> str:
        sess = self._sessions.get(name)
        if sess is None or not await self.deliverable(name):
            return "deferred"
        await sess["client"].request("turn/start", {
            "threadId": sess["thread_id"],
            "input": [{"type": "text", "text": text, "text_elements": []}],
        })
        return "sent"

    async def deliverable(self, name: str) -> bool:
        sess = self._sessions.get(name)
        if sess is None:
            return True  # sessao nao rastreada = nada em andamento pra bloquear
        return not sess["in_progress"]
