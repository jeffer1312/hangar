"""CodexAdapter: junta o rollout (historico, via parse_rollout_line) com o app-server (JSON-RPC
ao vivo, via AppServerClient) por tras do `Adapter` Protocol. O coracao e map_state(), que
traduz uma notification do app-server (camelCase) num resultado NEUTRO e testavel; state_monitor
consome isso e emite o StateEvent real do app.

Nomes/shapes confirmados contra codex-cli 0.141.0 em docs/codex-app-server-contract.md.

Task 5 completou o Adapter Protocol: alem de map_state/transcript_stream/state_monitor/send_prompt/
deliverable, agora tem drain/spawn_command/transcript_path + o lifecycle vivo (attach/ensure_running/
close_sync). ensure_running e o resume LAZY: pos-restart do backend o processo app-server morre mas
o sidecar duravel (app.adapters.codex.sessions) guarda thread_id/rollout_path/cwd -> ensure_running
reabre o AppServerClient e retoma pelo thread/resume sob demanda."""
import logging
from dataclasses import dataclass
from typing import AsyncIterator, Callable, Optional

from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex.appserver import AppServerClient
from app.adapters.codex.preview import CodexPreviewSource
from app.adapters.codex.rollout import parse_rollout_line
from app.pqueue import PromptQueue
from app.state import StateEvent
from app.transcript import ChatEvent, TranscriptTailer

_log = logging.getLogger("claude_pocket.codex.adapter")

# clientInfo do handshake initialize (ver docs/codex-app-server-contract.md).
_CLIENT_INFO = {"name": "claude-pocket", "title": None, "version": "0.1.0"}
# Codex pode EDITAR arquivos no cwd da sessao -> workspace-write (nao read-only do spike).
_SANDBOX = "workspace-write"
_APPROVAL = "never"


@dataclass
class MappedState:
    """Resultado CRU e testavel de map_state — NAO e o StateEvent do app (StateEvent exige
    `state` e nao tem campo de preview). state_monitor() traduz isto pro StateEvent real;
    preview_delta e consumido em paralelo por CodexAdapter._state_stream (acumula por turno e
    empurra pro CodexPreviewSource — ver Task 5b), fora do StateEvent."""
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
        """Liga uma sessao (por nome) a um AppServerClient + threadId ja vivos. Chamado pelo
        registry.create_codex (spawn novo) e por ensure_running (resume pos-restart)."""
        self._sessions[name] = {"client": client, "thread_id": thread_id,
                                 "state": "idle", "in_progress": False}

    async def ensure_running(self, name: str) -> Optional[AppServerClient]:
        """Garante um AppServerClient VIVO pra sessao Codex `name` (resume LAZY):
        - ja ha client vivo no dict -> retorna ele (caso quente).
        - senao, le o sidecar duravel; sem sidecar -> None (sessao Codex desconhecida).
        - com sidecar (pos-restart): reabre o app-server, initialize, e RETOMA o thread existente
          via `thread/resume` passando o threadId gravado (metodo confirmado no schema da 0.141.0;
          docstring do ThreadResumeParams: 'Prefer using thread_id whenever possible'). O historico
          nao se perde: ja esta no rollout JSONL; o resume so reconecta o processo vivo."""
        sess = self._sessions.get(name)
        if sess is not None:
            return sess["client"]
        meta = codex_sessions.load(name)
        if meta is None:
            return None
        client = AppServerClient()
        try:
            await client.start()
            await client.request("initialize", {"clientInfo": _CLIENT_INFO, "capabilities": None})
            result = await client.request("thread/resume", {
                "threadId": meta["thread_id"],
                "cwd": meta.get("cwd"),
                "sandbox": _SANDBOX,
                "approvalPolicy": _APPROVAL,
            })
        except Exception:
            # resume falhou (app-server morreu, thread perdido, etc.): nao deixa o subprocess orfao.
            await client.close()
            raise
        # thread/resume devolve {"thread": {"id","path",...}}; reusa o thread_id do sidecar como
        # fonte de verdade (o id nao muda no resume).
        thread_id = (result.get("thread") or {}).get("id") or meta["thread_id"]
        self.attach(name, client, thread_id)
        _log.info("codex ensure_running: resumed thread=%s name=%s", thread_id, name)
        return client

    def close_sync(self, name: str) -> None:
        """Encerramento SINCRONO do client vivo (chamado pelo registry.kill, que e sync). Manda
        SIGTERM best-effort no subprocess e esquece a sessao da memoria; o read loop (loop
        principal) ve o EOF e roda seu finally. NAO apaga o sidecar duravel -- isso e o kill()."""
        sess = self._sessions.pop(name, None)
        if sess is None:
            return
        term = getattr(sess["client"], "terminate", None)
        if callable(term):
            term()

    def transcript_stream(self, path: str) -> AsyncIterator[ChatEvent]:
        # Mesma mecanica de tail (backfill do tail + watch de append) do Claude, so trocando o
        # parser pro shape do rollout do Codex (snake_case, envelope {type, payload}).
        return TranscriptTailer(path, parse_line=parse_rollout_line).follow()

    def state_monitor(self, name: str, sid_get: Callable[[], str]) -> AsyncIterator[StateEvent]:
        return self._state_stream(name)

    async def _state_stream(self, name: str) -> AsyncIterator[StateEvent]:
        try:
            client = await self.ensure_running(name)
        except Exception:
            # resume falhou (app-server indisponivel) -> dead pro front em vez de derrubar o SSE.
            _log.exception("codex state_monitor: ensure_running falhou name=%s", name)
            client = None
        if client is None:
            # Sessao Codex desconhecida (sem client vivo e sem sidecar) -> "dead" pro front, igual
            # ao StateMonitor do Claude quando a sessao tmux some.
            yield StateEvent(session=name, state="dead")
            return
        sess = self._sessions[name]
        preview = CodexPreviewSource.get(name)
        # Buffer do turno em voo (deltas sao INCREMENTAIS -- concatena; ver docs/codex-app-server-
        # contract.md). Local ao generator: 1 state_monitor ativo por sessao no uso normal (1 SSE
        # aberto por sessao); N conexoes simultaneas cada uma chamaria ensure_running de novo e
        # teria seu proprio buffer, mas todas empurram pro MESMO CodexPreviewSource (registry por
        # nome) -- ainda convergem, so nao e o caso comum.
        # ponytail: buffer por-generator, nao por-sessao no adapter; multi-consumidor corrigido se
        # virar necessario (hoje o front so abre 1 SSE por sessao).
        buf = ""
        async for notif in client.notifications():
            mapped = map_state(notif)
            method = notif.get("method")
            if method == "turn/started":
                buf = ""  # novo turno -- zera pra nao vazar o texto do turno anterior
            elif mapped.preview_delta is not None:
                buf += mapped.preview_delta
                await preview.push(buf)
            elif method == "turn/completed":
                # o texto final ja caiu no rollout -> vira ChatEvent autoritativo via
                # transcript_stream; o sse.py tambem suprime via _already_committed. Limpa aqui pra
                # nao deixar o ultimo delta pendurado ate o proximo turno.
                await preview.push("")
            if mapped.state is not None:
                sess["state"] = mapped.state
                sess["in_progress"] = mapped.state == "working"
            if mapped.state is None and mapped.status_line is None:
                # Neutro (method desconhecido) ou so preview_delta: StateEvent nao tem campo de
                # preview -> nada a emitir aqui (o preview ja foi empurrado acima, fora do
                # StateEvent -- efeito colateral adicional, nao substitui).
                continue
            yield StateEvent(session=name, state=sess["state"], status_line=mapped.status_line)
        # notifications() terminou = EOF do app-server (o read loop empurra o sentinela ao morrer).
        # Dead-detection (backlog T4-m2): emite dead pra o front + limpa a sessao da memoria (o
        # sidecar duravel fica; ensure_running reabre num acesso futuro). getattr: um client FAKE de
        # teste sem `closed` termina o stream sem simular morte -> nao emite dead.
        if getattr(client, "closed", False):
            sess["state"] = "dead"
            self._sessions.pop(name, None)
            yield StateEvent(session=name, state="dead")

    async def send_prompt(self, name: str, text: str) -> str:
        client = await self.ensure_running(name)
        if client is None or not await self.deliverable(name):
            return "deferred"
        await client.request("turn/start", {
            "threadId": self._sessions[name]["thread_id"],
            "input": [{"type": "text", "text": text, "text_elements": []}],
        })
        return "sent"

    async def deliverable(self, name: str) -> bool:
        # Predicado BARATO (nao spawna): so olha o in_progress cacheado. Sessao nao anexada = nada
        # em andamento -> True (send_prompt e quem chama ensure_running e realmente entrega).
        sess = self._sessions.get(name)
        if sess is None:
            return True
        return not sess["in_progress"]

    async def drain(self, name: str, path: str) -> int:
        """Entrega a fila duravel (PromptQueue keyed por nome) via send_prompt (turn/start). Sem
        tty/overlay como no Claude: claim-1-envia-1, para no primeiro `deferred` (turno em curso).
        Retorna quantas entregou. `path` (rollout) mantido por assinatura do Protocol; nao usado."""
        q = PromptQueue(name)
        if not any(e.get("delivered") is False for e in q.load()):
            return 0
        sent = 0
        while True:
            claimed = q.claim_undelivered(limit=1)
            if not claimed:
                return sent
            entry = claimed[0]
            try:
                result = await self.send_prompt(name, entry["text"])
            except Exception:
                _log.exception("codex drain: falha ao entregar entry=%s name=%s", entry.get("id"), name)
                return sent
            if result != "sent":
                # turno em curso / sessao indisponivel: reverte (nada foi enviado) e espera o proximo idle.
                try:
                    q.set_delivered(entry["id"], False)
                except OSError:
                    pass
                return sent
            sent += 1

    def spawn_command(self, cwd: str, session_id: str) -> list[str]:
        # Sessao Codex NAO nasce de um comando tmux -- nasce de thread/start no app-server
        # (registry.create_codex). registry.create ramifica por provider ANTES de chamar isto no
        # caminho Codex, entao chegar aqui e uso incorreto -> falha alto (Protocol honesto).
        raise NotImplementedError("Codex nao usa spawn_command; use registry.create_codex")

    def transcript_path(self, cwd: str, session_id: str) -> str:
        # O rollout path vem do thread/start (result.thread.path), gravado no sidecar -- nao ha como
        # derivar do cwd+id como no Claude. Nunca chamado no caminho Codex.
        raise NotImplementedError("Codex obtem o rollout path via thread/start, nao por derivacao")
