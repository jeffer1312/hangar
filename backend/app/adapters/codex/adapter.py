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
import asyncio
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
        # Lock por-nome pra ensure_running: sem isto, 2 chamadores concorrentes pro mesmo nome sem
        # client vivo (ex: SSE reconnect + /input logo apos restart) podiam ambos passar pelo `sess
        # is None`, ambos spawnar+resume, e o 2o attach() sobrescrever o 1o AppServerClient no dict
        # -- o 1o (subprocess + reader task) ficava orfao, nunca fechado.
        self._locks: dict[str, asyncio.Lock] = {}

    def attach(self, name: str, client: AppServerClient, thread_id: str,
               model: Optional[str] = None, effort: Optional[str] = None) -> None:
        """Liga uma sessao (por nome) a um AppServerClient + threadId ja vivos. Chamado pelo
        registry.create_codex (spawn novo, sem model/effort ainda -- sessao nova) e por
        ensure_running (resume pos-restart, passando model/effort lidos do sidecar -- Task C:
        a escolha sobrevive ao restart)."""
        self._sessions[name] = {"client": client, "thread_id": thread_id,
                                 "state": "idle", "in_progress": False,
                                 "model": model, "effort": effort}

    async def ensure_running(self, name: str) -> Optional[AppServerClient]:
        """Garante um AppServerClient VIVO pra sessao Codex `name` (resume LAZY):
        - ja ha client vivo no dict -> retorna ele (caso quente).
        - senao, le o sidecar duravel; sem sidecar -> None (sessao Codex desconhecida).
        - com sidecar (pos-restart): reabre o app-server, initialize, e RETOMA o thread existente
          via `thread/resume` passando o threadId gravado (metodo confirmado no schema da 0.141.0;
          docstring do ThreadResumeParams: 'Prefer using thread_id whenever possible'). O historico
          nao se perde: ja esta no rollout JSONL; o resume so reconecta o processo vivo.

        Lock por-nome (IMPORTANT 1): sem ele, 2 chamadores concorrentes pro mesmo nome sem client
        vivo spawnavam 2 AppServerClient e o 2o attach() sobrescrevia o 1o no dict, vazando o
        subprocess orfao do 1o. setdefault no dict de locks e seguro sem lock proprio: nao ha
        `await` entre o get e o set, entao nenhuma outra corrotina roda no meio (cooperativo)."""
        sess = self._sessions.get(name)
        if sess is not None:
            return sess["client"]
        lock = self._locks.setdefault(name, asyncio.Lock())
        async with lock:
            # Double-check: outro chamador pode ter terminado de spawnar enquanto esperavamos o lock.
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
            # model/effort (Task C): repovoa a escolha do sidecar no dict quente, senao o 1o
            # turn/start pos-restart perderia a escolha ate a proxima chamada de set_model.
            self.attach(name, client, thread_id, model=meta.get("model"), effort=meta.get("effort"))
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
                # guarda o turnId do turno em voo (turn/interrupt exige threadId+turnId).
                turn_id = ((notif.get("params") or {}).get("turn") or {}).get("id")
                if turn_id:
                    sess["turn_id"] = turn_id
            elif mapped.preview_delta is not None:
                buf += mapped.preview_delta
                await preview.push(buf)
            elif method == "turn/completed":
                # o texto final ja caiu no rollout -> vira ChatEvent autoritativo via
                # transcript_stream; o sse.py tambem suprime via _already_committed. Limpa aqui pra
                # nao deixar o ultimo delta pendurado ate o proximo turno.
                await preview.push("")
                # Marca idle ANTES de drenar (nao depender do thread/status/changed idle ter chegado
                # antes -- a ordem das notifications do app-server nao e garantida). A drain chama
                # send_prompt -> deliverable(), que le in_progress: se ficasse True aqui, deliverable
                # daria False, send_prompt viraria "deferred", a drain reverteria e a entrada
                # enfileirada ficaria presa pra sempre (perda silenciosa). Tambem zera o turn_id: o
                # turno morreu -> interrupt vira no-op em vez de mandar turn/interrupt de turno morto.
                sess["state"] = "idle"
                sess["in_progress"] = False
                sess["turn_id"] = None
                # drain-on-complete (P2): turno terminou -> entrega a fila pendente (msgs enviadas
                # via /input enquanto o Codex trabalhava). Reusa adapter.drain (claim-1-envia-1 via
                # turn/start). ACOPLADO ao SSE ativo -- este generator so roda com um consumidor
                # aberto; sem celular conectado nao ha drain-on-complete (mesma limitacao do preview,
                # ver ponytail acima). Best-effort: falha aqui nunca derruba o state stream.
                try:
                    await self.drain(name, "")
                except Exception:
                    _log.exception("codex drain-on-complete falhou name=%s", name)
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
        sess = self._sessions[name]
        params = {
            "threadId": sess["thread_id"],
            "input": [{"type": "text", "text": text, "text_elements": []}],
        }
        # model/effort (Task C): so inclui quando ha escolha guardada -- omitido, o app-server usa
        # o default da thread. O schema documenta "override... AND subsequent turns": basta mandar
        # aqui de novo em CADA turn/start (nao ha metodo "set model" separado).
        if sess.get("model"):
            params["model"] = sess["model"]
        if sess.get("effort"):
            params["effort"] = sess["effort"]
        result = await client.request("turn/start", params)
        # guarda o turnId do turno recem-iniciado (turn/interrupt exige threadId+turnId; ver schema
        # TurnInterruptParams). turn/start devolve {"turn": {"id", ...}}.
        turn_id = (result.get("turn") or {}).get("id")
        if turn_id:
            sess["turn_id"] = turn_id
        # Marca in_progress AQUI (nao so esperar o turn/started chegar no loop de notifications):
        # o drain roda dentro desse mesmo loop, entao um turn/started concorrente pode nao ser
        # processado a tempo -- sem isto, deliverable() ficaria True e o drain mandaria todas as
        # entradas pendentes como turn/start back-to-back (ver test_drain_stops_after_first_delivery).
        sess["in_progress"] = True
        return "sent"

    async def interrupt(self, name: str) -> bool:
        """Interrompe o turno em curso via app-server turn/interrupt (exige threadId+turnId, shape
        confirmado no schema TurnInterruptParams da 0.141.0). No-op seguro (retorna False, nunca
        levanta) se a sessao nao tem client vivo ou nao ha turno em voo -- o endpoint /interrupt
        trata isso como ok pro Codex (nao quebra)."""
        sess = self._sessions.get(name)
        if sess is None:
            return False
        turn_id = sess.get("turn_id")
        if not turn_id:
            return False
        try:
            await sess["client"].request("turn/interrupt", {
                "threadId": sess["thread_id"], "turnId": turn_id,
            })
        except Exception:
            _log.exception("codex interrupt falhou name=%s", name)
            return False
        return True

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
        Retorna quantas entregou. `path` (rollout) mantido por assinatura do Protocol; nao usado.

        IMPORTANT 2: PromptQueue.load/claim_undelivered/set_delivered fazem I/O de arquivo SINCRONO
        com lock -- chamados direto numa corrotina bloqueariam o event loop (que serve o SSE de
        outras sessoes). Mesmo padrao do ClaudeAdapter (ver adapters/claude.py): to_thread."""
        q = PromptQueue(name)
        if not any(e.get("delivered") is False for e in await asyncio.to_thread(q.load)):
            return 0
        sent = 0
        while True:
            claimed = await asyncio.to_thread(q.claim_undelivered, limit=1)
            if not claimed:
                return sent
            entry = claimed[0]
            try:
                result = await self.send_prompt(name, entry["text"])
            except Exception:
                _log.exception("codex drain: falha ao entregar entry=%s name=%s", entry.get("id"), name)
                # CRITICAL: claim_undelivered ja marcou delivered=True (otimista). send_prompt
                # levantou (app-server morto/timeout/RuntimeError do JSON-RPC) -- sem reverter, a
                # entrada fica delivered=True pra sempre (nunca reenviada, bolha "queued-" eterna).
                # Mesmo tratamento do branch "deferred" abaixo.
                try:
                    await asyncio.to_thread(q.set_delivered, entry["id"], False)
                except OSError:
                    pass
                return sent
            if result != "sent":
                # turno em curso / sessao indisponivel: reverte (nada foi enviado) e espera o proximo idle.
                try:
                    await asyncio.to_thread(q.set_delivered, entry["id"], False)
                except OSError:
                    pass
                return sent
            sent += 1

    async def read_rate_limits(self, name: str) -> Optional[dict]:
        """Le os limites de uso da conta Codex via `account/rateLimits/read` (Task B). Devolve o
        `RateLimitSnapshot` cru (limitId/limitName/primary/secondary/credits/individualLimit/
        planType/rateLimitReachedType) ou None se a sessao nao tem client vivo/sidecar (mesmo
        contrato de ensure_running) ou se o app-server recusar o pedido -- nunca levanta, o
        endpoint trata None como "sem dado" em vez de derrubar a request."""
        try:
            client = await self.ensure_running(name)
        except Exception:
            _log.exception("codex read_rate_limits: ensure_running falhou name=%s", name)
            return None
        if client is None:
            return None
        try:
            result = await client.request("account/rateLimits/read", {})
        except Exception:
            _log.exception("codex read_rate_limits: request falhou name=%s", name)
            return None
        return result.get("rateLimits")

    async def list_models(self, name: str) -> list[dict]:
        """Lista os modelos disponiveis pra sessao via `model/list` (Task C), normalizados:
        {model, displayName, description, efforts: [{value, description}], defaultEffort}.
        Filtra hidden=True (schema 0.141.0: `Model.hidden`). Mesmo contrato de nao-levantar de
        read_rate_limits -- lista vazia se a sessao nao tem client vivo/sidecar ou o app-server
        recusar o pedido."""
        try:
            client = await self.ensure_running(name)
        except Exception:
            _log.exception("codex list_models: ensure_running falhou name=%s", name)
            return []
        if client is None:
            return []
        try:
            result = await client.request("model/list", {})
        except Exception:
            _log.exception("codex list_models: request falhou name=%s", name)
            return []
        return [
            {
                "model": m.get("model"),
                "displayName": m.get("displayName"),
                "description": m.get("description"),
                "efforts": [
                    {"value": e.get("reasoningEffort"), "description": e.get("description")}
                    for e in (m.get("supportedReasoningEfforts") or [])
                ],
                "defaultEffort": m.get("defaultReasoningEffort"),
            }
            for m in (result.get("data") or [])
            if not m.get("hidden")
        ]

    async def set_model(self, name: str, model: Optional[str], effort: Optional[str]) -> None:
        """Grava a escolha de modelo/effort da sessao (Task C): dict quente (se anexada) + sidecar
        duravel (sobrevive ao restart). NAO manda nada pro app-server agora -- vale a partir do
        PROXIMO turn/start (send_prompt inclui a escolha nos params; ver descoberta-chave no
        brief: turn/start "override... AND subsequent turns", sem metodo "set model" separado)."""
        sess = self._sessions.get(name)
        if sess is not None:
            sess["model"] = model
            sess["effort"] = effort
        codex_sessions.update_model(name, model, effort)

    def current_model(self, name: str) -> dict:
        """Modelo/effort escolhidos pra sessao: dict quente primeiro (mais recente), senao o
        sidecar duravel (sessao conhecida mas nao anexada agora); {model: None, effort: None} se
        nunca escolhido -- o proximo turn/start usa o default da thread."""
        sess = self._sessions.get(name)
        if sess is not None and (sess.get("model") is not None or sess.get("effort") is not None):
            return {"model": sess.get("model"), "effort": sess.get("effort")}
        meta = codex_sessions.load(name)
        if meta is not None:
            return {"model": meta.get("model"), "effort": meta.get("effort")}
        return {"model": None, "effort": None}

    def spawn_command(self, cwd: str, session_id: str) -> list[str]:
        # Sessao Codex NAO nasce de um comando tmux -- nasce de thread/start no app-server
        # (registry.create_codex). registry.create ramifica por provider ANTES de chamar isto no
        # caminho Codex, entao chegar aqui e uso incorreto -> falha alto (Protocol honesto).
        raise NotImplementedError("Codex nao usa spawn_command; use registry.create_codex")

    def transcript_path(self, cwd: str, session_id: str) -> str:
        # O rollout path vem do thread/start (result.thread.path), gravado no sidecar -- nao ha como
        # derivar do cwd+id como no Claude. Nunca chamado no caminho Codex.
        raise NotImplementedError("Codex obtem o rollout path via thread/start, nao por derivacao")
