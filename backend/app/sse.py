import asyncio
import json
from app.transcript import TranscriptTailer
from app.state import StateMonitor
from app.pqueue import PromptQueue, _transcript_start_ts
from app.preview import PreviewBroker, _norm
from app.models import PreviewEvent
from app.registry import SessionRegistry
from app.askquestion import read_pending_askq
from app.terminal_input import drain


def _ask_question_event(state_json: str, jsonl: str) -> dict | None:
    """Retorna o evento SSE ask_question p/ o AskUserQuestion MULTI-pergunta (tabbed), ou None.
    Dispara em awaiting_input + sidecar do hook com >=2 perguntas cujas opcoes batem com o menu atual."""
    try:
        obj = json.loads(state_json)
    except (json.JSONDecodeError, ValueError):
        return None
    if obj.get("state") != "awaiting_input":
        return None
    payload = read_pending_askq(jsonl)
    # 1 pergunta: o TUI submete direto no Enter da opcao (sem tela de Review) -> cai no OptionButtons
    # (menu de lista unica, non-goal do spec). So multi-pergunta abre o stepper.
    if payload is None or len(payload.questions) < 2:
        return None
    # NAO depende de `overlay`: is_overlay e fragil p/ AskUserQuestion — o rodape de navegacao sai das
    # ultimas 8 linhas do pane (linhas em branco no fim) -> overlay=False -> o stepper NUNCA abria e caia
    # no OptionButtons. Freshness pelo SIDECAR x menu atual: o sidecar nao e limpo se respondido pela TUI
    # (so no /answer + kill), entao confere que as opcoes da 1a pergunta batem com as do menu corrente
    # (classify) -> sidecar velho sobre OUTRO prompt (ex: permissao) nao abre o stepper.
    # ponytail: opcao truncada no pane faria o subset falhar -> degrada pro OptionButtons (= hoje), sem regressao.
    first_opts = {o.label for o in payload.questions[0].options}
    state_opts = set(obj.get("options") or [])
    if not first_opts or not first_opts <= state_opts:
        return None
    return {"event": "ask_question", "data": json.dumps(payload.model_dump(), ensure_ascii=False)}

# Stateless (so projects_dir) — usado pelo watcher pra detectar troca de jsonl (ex: /clear abre um
# transcript novo, mas a conexao SSE foi bindada no antigo).
_registry = SessionRegistry()

# Instancia stateless pro stream de lista (separada do _registry do jsonl_watcher pra clareza).
_list_registry = SessionRegistry()


async def list_events(poll: float = 1.5, ping_every: int = 7):
    """SSE da LISTA de sessoes. Emite o snapshot de list_with_state() na conexao e, num loop de
    `poll`s, reemite SO quando o resultado muda (estado via markers do A; membership por re-listar).
    Heartbeat 'ping' a cada `ping_every` ticks (alimenta o watchdog do front). Fail-loud: excecao
    do list_with_state propaga e encerra o stream (o EventSource do cliente reconecta)."""
    last = None
    ticks = 0
    while True:
        infos = await _list_registry.list_with_state()
        data = json.dumps([i.model_dump(mode="json") for i in infos], ensure_ascii=False)
        if data != last:
            last = data
            yield {"event": "sessions", "data": data}
        ticks += 1
        if ping_every and ticks % ping_every == 0:
            yield {"event": "ping", "data": "{}"}
        await asyncio.sleep(poll)


async def merged_events(name: str, jsonl: str):
    monitor = StateMonitor(name)
    pqueue = PromptQueue(name)
    broker = PreviewBroker.get(name)
    # Inicio da sessao atual: poda entradas de fila pre-/clear no live SSE (mesma regra do history).
    start_ts = _transcript_start_ts(jsonl)
    queue: asyncio.Queue = asyncio.Queue()
    # Slot coalescido do preview: NUNCA entra na FIFO compartilhada (firehose atrasaria o assistant_msg
    # autoritativo — head-of-line). Mantemos so o ULTIMO texto + um unico marcador pendente na fila;
    # frames intermediarios sao descartados (full-replace, o ultimo vence). Sem await entre as
    # escritas do dict -> consistente no loop asyncio single-thread, sem lock.
    preview_slot = {"text": "", "pending": False}
    # Texto da ULTIMA msg de assistente que já caiu no .jsonl (normalizado). Fonte de verdade pra
    # suprimir preview JÁ COMMITADO: no gap entre blocos (durante tool-calls) o pane ainda mostra o
    # bloco que já foi gravado -> sem isto, vira bolha duplicada. Atualizado pelo tail_pump.
    committed = {"text": ""}

    def _already_committed(text: str) -> bool:
        n = _norm(text)
        return len(n) >= 16 and bool(committed["text"]) and n in committed["text"]

    async def pump(kind, agen):
        try:
            async for item in agen:
                # model_dump_json (not model_dump): the SSE `data:` field must be a
                # JSON string for the browser's JSON.parse(e.data). A raw dict gets
                # str()'d by sse-starlette into Python repr (None/single quotes) = invalid JSON.
                await queue.put((kind, item.model_dump_json()))
        except Exception as exc:  # surface, never swallow
            await queue.put(("__error__", exc))

    async def ping_loop():
        # Heartbeat VISIVEL pro cliente (a cada 10s). Diferente do ping interno do sse_starlette,
        # que vai como COMENTARIO (': ping') e o EventSource ignora -> o front nao consegue observar.
        # Este vai como evento real 'ping' pra alimentar o watchdog de liveness do front: numa
        # conexao half-open (mobile troca de rede / app no background), sem isto o front congela no
        # ultimo estado pq nada chega e o onerror nao dispara. O ping faz o front detectar e reconectar.
        while True:
            await asyncio.sleep(10)
            await queue.put(("ping", "{}"))

    def _enqueue_preview(text: str):
        # Atualiza o slot e enfileira UM marcador 'preview' por vez (drop-old). Sem await entre as
        # escritas -> consistente no loop single-thread.
        preview_slot["text"] = text
        if not preview_slot["pending"]:
            preview_slot["pending"] = True
            queue.put_nowait(("preview", None))

    async def tail_pump(path: str):
        # Transcript do .jsonl (msgs canonicas). Alem de emitir, RASTREIA a ultima msg de assistente
        # em `committed` -> fonte de verdade pra suprimir preview duplicado. E quando um bloco commita
        # que e exatamente o que o preview mostra, LIMPA o preview na hora (sem esperar o broker mudar).
        # Recebe o path (em vez de fechar sobre um tailer fixo) pra poder ser recriado no rebind do /clear.
        try:
            async for ev in TranscriptTailer(path).follow():
                if ev.kind == "assistant_msg" and ev.text:
                    committed["text"] = _norm(ev.text)
                    if _already_committed(preview_slot["text"]):
                        _enqueue_preview("")
                await queue.put(("message", ev.model_dump_json()))
        except asyncio.CancelledError:
            raise  # rebind do watcher cancela este task de proposito -> nao reportar como erro
        except Exception as exc:  # surface, never swallow
            await queue.put(("__error__", exc))

    async def jsonl_watcher():
        # Detecta /clear (e qualquer troca de transcript): o claude abre um .jsonl NOVO, mas o tailer foi
        # bindado no antigo -> nada novo chegaria ate o EventSource reconectar (o usuario tinha que sair e
        # voltar). Aqui, vigia o jsonl ATIVO desta sessao e, quando diverge do bindado, sinaliza reset.
        # IMPORTANTE: usa a MESMA resolucao do endpoint /events (registry.list -> resolve()): cmdline
        # --session-id, depois fd aberto, depois btime, depois newest-by-mtime. Espelhar o endpoint
        # garante que o watcher dispare exatamente quando um reconnect mudaria de transcript.
        current = jsonl
        pending = None       # candidato a nova resolucao, aguardando confirmar persistencia
        pending_n = 0
        while True:
            await asyncio.sleep(2)
            try:
                live = next((s.jsonl for s in await asyncio.to_thread(_registry.list) if s.name == name), None)
            except Exception:
                live = None
            if not live or live == current:
                pending = None
                pending_n = 0
                continue
            # Mudou: exige PERSISTIR por >=2 polls antes de resetar. Filtra flips transitorios (a
            # resolucao oscila quando o processo com --session-id some por 1 ciclo) que limpavam o chat.
            pending_n = pending_n + 1 if live == pending else 1
            pending = live
            if pending_n >= 2:
                current = live
                pending = None
                pending_n = 0
                queue.put_nowait(("__reset__", live))

    async def preview_pump():
        # Assina o broker COMPARTILHADO da sessao (1 loop de capture pra N conexoes). Coalesce (slot +
        # 1 marcador). SUPRIME texto JA COMMITADO no .jsonl (gap entre blocos) -> manda "" pra nao
        # duplicar. Fail-loud como os outros pumps.
        try:
            async for text in broker.subscribe():
                _enqueue_preview("" if _already_committed(text) else text)
        except Exception as exc:  # surface, never swallow
            await queue.put(("__error__", exc))

    current_jsonl = jsonl          # atualizado no __reset__ (ex: /clear abre novo transcript)
    ask_q_emitted = False          # impede reemissao enquanto o mesmo prompt permanece na tela
    prev_deliverable = False     # init False -> 1o estado entregavel pos-(re)connect tambem dispara 1
                                 # drain (recovery de restart/reconexao com pendencia)
    drain_tasks: set = set()     # drains fire-and-forget; NAO entram em `tasks` (nao cancelar no disconnect)

    tail_task = asyncio.create_task(tail_pump(jsonl))
    tasks = [
        tail_task,
        # Fila duravel: user_msg sinteticos (id "queued-") pras msgs enfileiradas. O front faz o
        # dedup cruzado (queued- vs real) por texto.
        asyncio.create_task(pump("message", pqueue.follow(min_ts=start_ts))),
        asyncio.create_task(pump("state", monitor.stream())),
        asyncio.create_task(ping_loop()),
        asyncio.create_task(preview_pump()),
        asyncio.create_task(jsonl_watcher()),
    ]
    try:
        while True:
            event, data = await queue.get()
            if event == "__error__":
                raise data
            if event == "__reset__":
                # Troca de transcript (ex: /clear). Re-binda o tailer no jsonl novo, zera o estado de
                # suppress/preview, e manda 'reset' pro front recarregar o history do zero.
                tasks.remove(tail_task)
                tail_task.cancel()
                committed["text"] = ""
                _enqueue_preview("")
                current_jsonl = data
                ask_q_emitted = False
                tail_task = asyncio.create_task(tail_pump(data))
                tasks.append(tail_task)
                yield {"event": "reset", "data": "{}"}
                continue
            if event == "preview":
                # Le o ULTIMO texto do slot na hora do envio (frames antigos ja foram sobrescritos).
                # SEM id: pra reconexao do EventSource nao replayar preview velho via Last-Event-ID.
                preview_slot["pending"] = False
                yield {"event": "preview",
                       "data": PreviewEvent(session=name, text=preview_slot["text"]).model_dump_json()}
                continue
            if event == "state":
                # Rastreia transicoes do awaiting_input pra resetar o guard de emissao unica.
                # Quando awaiting_input + overlay (rodape de abas = AskUserQuestion estruturado),
                # emite ask_question UMA VEZ por prompt; reseta ao sair do estado.
                parsed_state = json.loads(data)
                if parsed_state.get("state") != "awaiting_input":
                    ask_q_emitted = False
                elif not ask_q_emitted:
                    ask_ev = _ask_question_event(data, current_jsonl)
                    if ask_ev:
                        ask_q_emitted = True
                        yield ask_ev
                # Drain gatilho: quando o pane volta a aceitar texto livre (overlay/menu fechou, ou a
                # sessao voltou ao idle), entrega as enfileiradas pendentes. Deriva a entregabilidade
                # dos campos do PROPRIO StateEvent — reusa o stream do StateMonitor, sem novo poll.
                deliverable_now = (
                    parsed_state.get("state") not in ("awaiting_input", "dead")
                    and not parsed_state.get("overlay")
                )
                if deliverable_now and not prev_deliverable:
                    # fire-and-forget no threadpool (drain bloqueia em send_prompt) — nunca await no
                    # loop SSE. FORA de `tasks`: deixar um drain em voo terminar apos o phone
                    # desconectar e correto (entrega duravel nao depende do phone ficar conectado).
                    dt = asyncio.create_task(asyncio.to_thread(drain, name, current_jsonl))
                    drain_tasks.add(dt)
                    dt.add_done_callback(drain_tasks.discard)
                prev_deliverable = deliverable_now
            yield {"event": event, "data": data}
    finally:
        # So cancela e retorna (NAO await): um pump preso num asyncio.to_thread (tmux) nao e
        # cancelavel -> aguardar o gather aqui travava o aclose() do gerador, segurava a conexao
        # meio-aberta e, em rajada de reconexao do mobile, ia acumulando ate exaurir o threadpool
        # (a /api/sessions travava). Os inotify saem no GC; melhor isso que travar o disconnect.
        for t in tasks:
            t.cancel()
