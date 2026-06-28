import asyncio
import json
from app.transcript import TranscriptTailer
from app.state import StateMonitor
from app.pqueue import PromptQueue, _transcript_start_ts
from app.preview import PreviewBroker, _norm
from app.models import PreviewEvent
from app.registry import SessionRegistry
from app.askquestion import read_pending_askq


def _ask_question_event(state_json: str, jsonl: str) -> dict | None:
    """Retorna o evento SSE ask_question quando o estado for awaiting_input com overlay
    (rodape de navegacao por abas, indicando AskUserQuestion estruturado). Caso contrario, None."""
    try:
        obj = json.loads(state_json)
    except (json.JSONDecodeError, ValueError):
        return None
    if obj.get("state") != "awaiting_input" or not obj.get("overlay"):
        return None
    payload = read_pending_askq(jsonl)
    if payload is None:
        return None
    return {"event": "ask_question", "data": json.dumps(payload.model_dump(), ensure_ascii=False)}

# Stateless (so projects_dir) — usado pelo watcher pra detectar troca de jsonl (ex: /clear abre um
# transcript novo, mas a conexao SSE foi bindada no antigo).
_registry = SessionRegistry()


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
            yield {"event": event, "data": data}
    finally:
        # So cancela e retorna (NAO await): um pump preso num asyncio.to_thread (tmux) nao e
        # cancelavel -> aguardar o gather aqui travava o aclose() do gerador, segurava a conexao
        # meio-aberta e, em rajada de reconexao do mobile, ia acumulando ate exaurir o threadpool
        # (a /api/sessions travava). Os inotify saem no GC; melhor isso que travar o disconnect.
        for t in tasks:
            t.cancel()
