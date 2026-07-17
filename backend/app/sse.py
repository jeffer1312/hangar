import asyncio
import json
import re
import time
from pathlib import Path
from app.adapters import get_adapter
from app.adapters.codex.preview import CodexPreviewSource
from app.pqueue import PromptQueue, _transcript_start_ts
from app.preview import PreviewBroker, _norm
from app.models import PreviewEvent
from app.registry import SessionRegistry
from app.askquestion import read_pending_askq


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
    if payload is None:
        return None
    # 1 pergunta: o TUI submete direto no Enter da opcao (sem tela de Review) -> cai no OptionButtons
    # (menu de lista unica, non-goal do spec). So multi-pergunta abre o stepper — EXCETO quando alguma
    # opcao tem `preview` (codigo/mockup ao lado): o OptionButtons nao tem o payload, so o stepper
    # renderiza o preview. answer_questions ja trata pergunta unica (Enter da selecao submete).
    has_preview = any(o.preview for q in payload.questions for o in q.options)
    if len(payload.questions) < 2 and not has_preview:
        return None
    # NAO depende de `overlay`: is_overlay e fragil p/ AskUserQuestion — o rodape de navegacao sai das
    # ultimas 8 linhas do pane (linhas em branco no fim) -> overlay=False -> o stepper NUNCA abria e caia
    # no OptionButtons. Freshness pelo SIDECAR x menu atual: o sidecar nao e limpo se respondido pela TUI
    # (so no /answer + kill), entao confere que as opcoes da 1a pergunta batem com as do menu corrente
    # (classify) -> sidecar velho sobre OUTRO prompt (ex: permissao) nao abre o stepper.
    # Freshness: sem preview, igualdade exata (sidecar ⊆ pane) — protecao original contra sidecar
    # STALE abrir o stepper sobre OUTRO prompt (ex: menu de permissao). COM preview, a label do pane
    # vem truncada pelo wrap da coluna ("System no topo (igual aos" vs "...igual aos irmãos)") ->
    # relaxa pra prefixo NUMA DIRECAO SO (opcao do pane e prefixo da label completa; o inverso
    # deixaria label curta "Yes" casar com "Yes, and bypass permissions" = cross-wire de permissao)
    # + contagem igual de opcoes. Falhou -> degrada pro OptionButtons (= hoje), sem regressao.
    first_opts = {o.label for o in payload.questions[0].options}
    state_opts = set(obj.get("options") or [])
    if not first_opts or not state_opts:
        return None
    if not has_preview:
        if not first_opts <= state_opts:
            return None
    else:
        def _match(lbl: str) -> bool:
            return any(s and lbl.startswith(s) for s in state_opts)
        if len(first_opts) != len(state_opts) or not all(_match(l) for l in first_opts):
            return None
    return {"event": "ask_question", "data": json.dumps(payload.model_dump(), ensure_ascii=False)}

# Stateless (so projects_dir) — usado pelo watcher pra detectar troca de jsonl (ex: /clear abre um
# transcript novo, mas a conexao SSE foi bindada no antigo).
_registry = SessionRegistry()

# Instancia stateless pro stream de lista (separada do _registry do jsonl_watcher pra clareza).
_list_registry = SessionRegistry()

# Snapshot compartilhado de registry.list() pros LOOPS do SSE (jsonl_watcher de cada conexao de chat
# + list_events): cada um re-varria o /proc inteiro + tmux no proprio ciclo -> N conexoes = N
# varreduras completas a cada ~2s. Com TTL < poll dos consumidores, vira no maximo ~1 varredura/s no
# total, sem atraso percebido. Endpoints request/response seguem chamando registry.list() fresco.
# ponytail: check-then-set sem lock (dois callers no vencimento do TTL = 2 scans, igual a hoje);
# lock de asyncio aqui arriscaria bind em event loop errado nos testes.
_LIST_TTL = 1.0
_list_snap: dict = {"t": 0.0, "infos": None}


async def _cached_list():
    now = time.monotonic()
    if _list_snap["infos"] is not None and now - _list_snap["t"] < _LIST_TTL:
        return _list_snap["infos"]
    infos = await asyncio.to_thread(_registry.list)
    _list_snap["infos"] = infos
    _list_snap["t"] = time.monotonic()
    return infos


# Reducao ESTAVEL da statusline pro dedup da lista: modelo, contexto em baldes de 5%, ⚡5h% e 📅7d%.
# Relogio (⏱) e custo ficam DE FORA — mudam a cada captura e re-emitiriam a lista inteira a toa.
# Espelha o parse do front (frontend/src/lib/statusline.ts), so o subset que o sig precisa.
_ST_MODEL = re.compile(r"🤖\s*([^(│]+)")
_ST_5H = re.compile(r"⚡[^│]*?(\d+)\s*%")
_ST_7D = re.compile(r"📅[^│]*?(\d+)\s*%")
_ST_PAIR = re.compile(r"([\d.,]+)\s*([kKmM])?\s*/\s*([\d.,]+)\s*([kKmM])?")


def _status_sig(s):
    if not s:
        return None
    ctx = None
    seg = re.search(r"💬([^│]*)", s)
    if seg:
        pairs = _ST_PAIR.findall(seg.group(1))
        # >=2 pares: o 1o e in/out do turno; o ULTIMO e uso/janela (mesma regra do front).
        if len(pairs) >= 2:
            def _num(x, unit):
                mult = {"k": 1e3, "m": 1e6}.get((unit or "").lower(), 1.0)
                try:
                    return float(x.replace(",", "")) * mult
                except ValueError:
                    return 0.0
            u, uu, t, tu = pairs[-1]
            total = _num(t, tu)
            if total > 0:
                ctx = round(_num(u, uu) / total * 20)  # baldes de 5% (round: 4.9999… nao vira 4)
    return (
        m.group(1).strip() if (m := _ST_MODEL.search(s)) else None,
        ctx,
        m.group(1) if (m := _ST_5H.search(s)) else None,
        m.group(1) if (m := _ST_7D.search(s)) else None,
    )


async def list_events(poll: float = 1.5, ping_every: int = 7):
    """SSE da LISTA de sessoes. Emite o snapshot de list_with_state() na conexao e, num loop de
    `poll`s, reemite SO quando o resultado muda (estado via markers do A; membership por re-listar).
    Heartbeat 'ping' a cada `ping_every` ticks (alimenta o watchdog do front). Fail-loud: excecao
    do list_with_state propaga e encerra o stream (o EventSource do cliente reconecta)."""
    last = None
    ticks = 0
    while True:
        # Resolucao via snapshot compartilhado (copias: list_with_state MUTA state/last_activity e o
        # cache e compartilhado com os jsonl_watchers); a classificacao de estado roda fresca.
        snap = [i.model_copy() for i in await _cached_list()]
        infos = await _list_registry.list_with_state(snap)
        data = json.dumps([i.model_dump(mode="json") for i in infos], ensure_ascii=False)
        # Dedup IGNORA last_activity: e o mtime do jsonl (float sub-segundo) que muda a CADA escrita
        # de uma sessao ativa -> sem isto a lista inteira re-emitia a cada poll (~1.5s) sem nada
        # visivel mudar = flicker no front. last_activity so e tiebreak de ordenacao + tempo relativo
        # coarse (minutos) no switcher; uns segundos defasado e invisivel. Re-emite so em mudanca de
        # membership/state/cwd/tracked/jsonl/question/stalled/limited. O payload ainda CARREGA
        # last_activity (so nao dispara). question entra pra a linha atualizar quando uma sessao passa a
        # perguntar algo novo; stalled (feature #7) entra pra a linha tingir/destingir quando o watchdog
        # flipar o bool; limited + limit_reset (feature #8) entram pro chip "limitado · HH:MM" aparecer/
        # sumir E re-emitir quando so o horario de reset muda (mesmo limited seguindo True);
        # then_target (feature #12) entra pro indicador de vinculo aparecer/sumir/trocar de alvo na hora.
        # status_line entra REDUZIDO (_status_sig): a linha crua carrega relogio/custo que mudam a
        # cada captura — o payload leva a crua, o sig so o que muda de verdade (modelo/ctx/5h/7d).
        # bool(label): a PRESENÇA da barrinha de spinner precisa re-emitir (sem isto o sweep
        # capturava o label e ninguém avisava o front); o TEXTO fica fora — muda a cada captura
        # (timer/tokens) e re-emitiria a lista inteira à toa.
        sig = json.dumps(
            [(i.name, i.cwd, i.state, i.tracked, i.jsonl, i.question, i.stalled, i.limited,
              i.limit_reset, i.then_target, _status_sig(getattr(i, "status_line", None)),
              bool(getattr(i, "label", None)))
             for i in infos],
            ensure_ascii=False,
        )
        if sig != last:
            last = sig
            yield {"event": "sessions", "data": data}
        ticks += 1
        if ping_every and ticks % ping_every == 0:
            yield {"event": "ping", "data": "{}"}
        await asyncio.sleep(poll)


async def merged_events(name: str, jsonl: str, provider: str = "claude"):
    # provider: default "claude" preserva o comportamento de hoje pros callers que ainda nao passam
    # (api.py so passa quando uma tarefa futura ligar o seletor de provider no endpoint).
    adapter = get_adapter(provider)
    current_jsonl = jsonl          # atualizado no __reset__ (ex: /clear abre novo transcript)
    # Ancora de hook do estado: o monitor le o marcador do sid VIVO (a closure acompanha o rebind
    # do /clear, que troca o current_jsonl -> sid novo).
    monitor_stream = adapter.state_monitor(
        name, sid_get=lambda: Path(current_jsonl).stem if current_jsonl else None)
    pqueue = PromptQueue(name)
    # Fonte do preview ao vivo ramifica por provider: Claude nao tem push (o app-server manda os
    # deltas, o TUI do Claude nao) -> continua no PreviewBroker (poll do pane). Codex nao tem pane
    # -> CodexPreviewSource, alimentado por push do CodexAdapter.state_monitor. Mesma interface
    # publica (get/subscribe) -> o resto do pump (preview_pump/_enqueue_preview/_already_committed)
    # fica IGUAL pras duas fontes.
    broker = CodexPreviewSource.get(name) if provider == "codex" else PreviewBroker.get(name)
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
            async for ev in adapter.transcript_stream(path):
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
                live = next((s.jsonl for s in await _cached_list() if s.name == name), None)
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
        # Assina a fonte COMPARTILHADA da sessao (1 broker pra N conexoes: PreviewBroker faz 1 loop
        # de capture do pane; CodexPreviewSource so guarda o ultimo push, sem loop). Coalesce (slot +
        # 1 marcador). SUPRIME texto JA COMMITADO no .jsonl (gap entre blocos) -> manda "" pra nao
        # duplicar. Fail-loud como os outros pumps.
        try:
            async for text in broker.subscribe():
                _enqueue_preview("" if _already_committed(text) else text)
        except Exception as exc:  # surface, never swallow
            await queue.put(("__error__", exc))

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
        asyncio.create_task(pump("state", monitor_stream)),
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
                    # fire-and-forget (adapter.drain ja roda no threadpool internamente) — nunca await
                    # no loop SSE. FORA de `tasks`: deixar um drain em voo terminar apos o phone
                    # desconectar e correto (entrega duravel nao depende do phone ficar conectado).
                    dt = asyncio.create_task(adapter.drain(name, current_jsonl))
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
