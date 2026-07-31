import asyncio
import json
import logging
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


# O front deriva o uso de contexto do 2º par do segmento 💬 (`<in>/<out> <usado>/<janela>`) e, com
# só um par, mostra "medição indisponível" — corretamente, porque ler in/out como contexto daria
# 100% falso. O que faltava era saber POR QUE o par some: o statusline é produzido pelo script do
# usuário a partir do payload do Claude Code, que às vezes não traz context_window. Este log grava o
# statusline CRU nesses momentos, pra a causa sair de medição e não de chute.
_CTX_PAIR_RE = re.compile(r"([\d.,]+)\s*[kKmM]?\s*/\s*([\d.,]+)\s*[kKmM]?")


def context_pairs(status_line: str | None) -> int:
    """Quantos pares numéricos há no segmento 💬 do statusline (>=2 => há métrica de contexto)."""
    if not status_line:
        return 0
    seg = re.search(r"💬([^│]*)", status_line)
    return len(_CTX_PAIR_RE.findall(seg.group(1))) if seg else 0


def preview_is_committed(preview: str, committed: str) -> bool:
    """O texto do preview já é o bloco que caiu no .jsonl? (regra PURA, testável isolada do stream.)

    DOIS casos, não um:
      1. preview ⊆ commitado — no gap entre blocos o pane ainda mostra o bloco já gravado.
      2. commitado é PREFIXO do preview — o extract_assistant_text não cortou o chrome (verbo de
         status do Claude Code fora do _TOOL_VERBS, ex. "Making 1 scratchpad edit…") e ele grudou
         no fim da prosa já gravada.

    O caso 2 faltava: sem ele a bolha DUPLICAVA e ficava piscando (o preview repetia a mensagem
    anterior + a linha de status). Ele é o que sobrevive ao vocabulário do TUI mudar de novo — a
    lista de verbos é calibração best-effort, esta regra não depende dela.

    Piso de 16 chars: um trecho curto casa por acidente com qualquer coisa.
    """
    n = _norm(preview)
    if len(n) < 16 or not committed:
        return False
    return n in committed or n.startswith(committed)


# Linhas que o proprio TUI acrescenta a QUALQUER AskUserQuestion. Nunca vem no payload do hook, entao
# nao podem contar como "opcao a mais" na checagem de frescor abaixo.
_TUI_EXTRAS = frozenset({"Type something.", "Chat about this"})


def _ask_question_event(state_json: str, jsonl: str) -> dict | None:
    """Retorna o evento SSE ask_question que abre o stepper nativo do AskUserQuestion, ou None.
    Dispara em awaiting_input + sidecar do hook cujas opcoes batem com o menu corrente do pane —
    qualquer numero de perguntas, inclusive uma."""
    try:
        obj = json.loads(state_json)
    except (json.JSONDecodeError, ValueError):
        return None
    if obj.get("state") != "awaiting_input":
        return None
    payload = read_pending_askq(jsonl)
    if payload is None:
        return None
    # Pergunta UNICA tambem abre o stepper. Havia aqui um gate `len(questions) < 2 -> None`, com o
    # argumento de que o TUI submete direto no Enter (sem tela de Review) e o OptionButtons bastava.
    # O que ele custava: o OptionButtons le o picker do PANE, e o pane so tem ROTULO — a `description`
    # de cada opcao, que e onde mora a explicacao da escolha, sumia da tela. Como o padrao e perguntar
    # uma coisa por vez, isso valia pra quase toda pergunta. answer_questions ja trata pergunta unica
    # (terminal_input.py:463, com guard de malha fechada justamente porque ali o Enter ja submete).
    # `has_preview` sobrevive porque decide a ESTRATEGIA DE CASAMENTO logo abaixo, nao mais se emite.
    has_preview = any(o.preview for q in payload.questions for o in q.options)
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
    # As linhas que o TUI acrescenta saem da conta nos DOIS ramos, nao so no de baixo. No ramo COM
    # preview a comparacao e por CONTAGEM IGUAL, entao mante-las ali reprovava 100% das perguntas com
    # preview — exatamente o caminho que existe pra nao perder o preview. Bug anterior a este trecho:
    # o teste do ramo de preview usava um pane fabricado sem as extras e nunca o exercitou.
    pane_opts = set(obj.get("options") or []) - _TUI_EXTRAS
    if not first_opts or not pane_opts:
        return None
    if not has_preview:
        if not first_opts <= pane_opts:
            return None
        # Subset sozinho nao basta. Um sidecar STALE cujos rotulos por acaso APARECEM num menu maior
        # (ex: {Sim, Nao} contra um menu [Cancelar, Sim, Nao]) passava — e como answer_questions
        # submete POR INDICE, com o menu real em outra ordem o Enter cai na linha errada. Entao
        # nenhuma opcao REAL do pane pode faltar no sidecar. _TUI_EXTRAS sai da conta porque o TUI a
        # acrescenta a toda pergunta e ela nunca esta no payload do hook.
        # Se o Claude Code renomear essas linhas, o casamento passa a reprovar e a sessao degrada pro
        # OptionButtons — o comportamento antigo, nunca resposta na linha errada.
        #
        # O log e o que torna essa degradacao VISIVEL, e o sinal esta na FREQUENCIA: uma linha
        # ocasional e sidecar velho, que e o esperado; a mesma linha em TODA pergunta significa que
        # as linhas do TUI mudaram de texto e ninguem mais ve descricao de opcao. Sem isto o unico
        # sintoma seria "a tela ficou mais pobre", sem ninguem saber por que.
        # `_log` e definido adiante no modulo; resolve em tempo de chamada.
        if extras := pane_opts - first_opts:
            _log.info("askq: opcao no pane fora do sidecar, degrada p/ OptionButtons extras=%s", extras)
            return None
    else:
        def _match(lbl: str) -> bool:
            return any(s and lbl.startswith(s) for s in pane_opts)
        if len(first_opts) != len(pane_opts) or not all(_match(l) for l in first_opts):
            return None
    return {"event": "ask_question", "data": json.dumps(payload.model_dump(), ensure_ascii=False)}

# Stateless (so projects_dir) — usado pelo watcher pra detectar troca de jsonl (ex: /clear abre um
# transcript novo, mas a conexao SSE foi bindada no antigo).
_registry = SessionRegistry()

# Instancia stateless pro stream de lista (separada do _registry do jsonl_watcher pra clareza).
_list_registry = SessionRegistry()

_log = logging.getLogger("claude_pocket.sse")

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


def _list_sig(infos) -> str:
    # Dedup IGNORA last_activity: e o mtime do jsonl (float sub-segundo) que muda a CADA escrita de uma
    # sessao ativa -> sem isto a lista inteira re-emitia a cada poll sem nada visivel mudar = flicker.
    # Re-emite so em mudanca de membership/state/cwd/tracked/jsonl/question/stalled/limited/
    # limit_reset/then_target/status_line-reduzida/presenca-de-label/loop/engine. Sem o engine aqui,
    # resumir um pane cujo motor sumiu do engines.json (kimi -> None) nao reemite a lista e o chip
    # ⚙ kimi fica preso, calado.
    # Sem o plan_name aqui, trocar do plano A pro B com o mesmo 9/17 nao re-emite e o chip fica
    # preso no plano errado — mesmo bug do engine.
    # plan_tasks/plan_task/plan_task_total/plan_complete tambem entram: um step desmarcado na
    # Task 1 e outro marcado na Task 2 no mesmo write pode deixar done/total liquidos (e plan_name)
    # identicos e ainda assim mudar a distribuicao por Task — sem isto a barra segmentada e a
    # Task atual ficam com o snapshot velho ate outra coisa qualquer mudar a sig.
    return json.dumps(
        [(i.name, i.cwd, i.state, i.tracked, i.jsonl, i.question, i.stalled, i.limited,
          i.limit_reset, i.then_target, _status_sig(getattr(i, "status_line", None)),
          bool(getattr(i, "label", None)),
          getattr(i, "loop_status", None), getattr(i, "loop_iter", None),
          getattr(i, "engine", None),
          getattr(i, "plan_name", None), getattr(i, "plan_done", None),
          getattr(i, "plan_total", None),
          getattr(i, "plan_task", None), getattr(i, "plan_task_total", None),
          getattr(i, "plan_complete", None),
          tuple(map(tuple, getattr(i, "plan_tasks", None) or [])))
         for i in infos],
        ensure_ascii=False,
    )


class _ListRefresher:
    """UM refresher em background (single-flight, compartilhado por TODAS as conexoes da lista) que
    produz o snapshot de list_with_state no ritmo dele. Desenho (decisao do jefferson): a conexao SSE
    e PRIORIDADE ABSOLUTA e NUNCA espera trabalho — ela so LE o ultimo snapshot pronto e emite quando
    a versao muda; o custo de raspar/decorar mora AQUI, uma vez, nao M×conexoes. Decoracao que falha
    (git 2s pendurado etc.) e LOGADA (warning — isolamento sim, silencio nao) mantendo o snapshot
    anterior (stale > morto): refresher travado = clientes seguem pingando e vendo a lista velha, nunca
    desconectam. O ref-count para o refresher quando a ultima conexao sai (nao raspa tmux com zero clientes)."""

    def __init__(self, poll: float = 1.5):
        self.poll = poll
        self.data: str | None = None
        self.sig: str | None = None
        self.version = 0
        self.errored = False
        self._task: asyncio.Task | None = None
        self._refs = 0
        self._loop = None
        self._cond: asyncio.Condition | None = None

    def _ensure(self):
        # (Re)inicia o refresher se nao ha task viva NESTE event loop. O bind por-loop e o que deixa o
        # singleton sobreviver aos asyncio.run() dos testes (cada um e um loop novo).
        loop = asyncio.get_running_loop()
        if self._task is None or self._task.done() or self._loop is not loop:
            self._loop = loop
            self._cond = asyncio.Condition()
            self.data = None
            self.sig = None
            self.version = 0
            self.errored = False
            self._refs = 0
            self._task = asyncio.create_task(self._run())

    async def _run(self):
        while True:
            try:
                snap = [i.model_copy() for i in await _cached_list()]
                infos = await _list_registry.list_with_state(snap)
                data = json.dumps([i.model_dump(mode="json") for i in infos], ensure_ascii=False)
                sig = _list_sig(infos)
            except Exception:
                # Decoracao/raspagem falhou -> MANTEM o snapshot anterior (stale > morto), nunca derruba
                # a conexao. Loga (padrao da casa) E sinaliza 'list_error' UMA vez (na transicao) pras
                # conexoes — lista vazia por falha e indistinguivel de zero sessoes; o front distingue
                # "erro" de "offline". Ciclo bom seguinte re-emite 'sessions' e limpa o erro no front.
                _log.warning("refresher da lista falhou; mantem snapshot anterior", exc_info=True)
                if not self.errored:
                    async with self._cond:
                        self.errored = True
                        self.version += 1
                        self._cond.notify_all()
                await asyncio.sleep(self.poll)
                continue
            # sucesso: emite se a sig mudou OU se estava em erro (pra o front LIMPAR o list_error).
            if sig != self.sig or self.errored:
                async with self._cond:
                    self.errored = False
                    self.sig = sig
                    self.data = data
                    self.version += 1
                    self._cond.notify_all()
            await asyncio.sleep(self.poll)

    def acquire(self) -> asyncio.Condition:
        self._ensure()
        self._refs += 1
        return self._cond

    def release(self):
        self._refs -= 1
        if self._refs <= 0 and self._task is not None:
            self._task.cancel()
            self._task = None
            self._refs = 0


_list_refresher = _ListRefresher()


async def list_events(ping_secs: float = 8.0):
    """SSE da LISTA de sessoes. Conexao = PRIORIDADE ABSOLUTA, zero trabalho: um reader que so LE o
    snapshot compartilhado (produzido pelo _ListRefresher unico) e emite quando a versao muda, + um
    ping em timer FIXO por conexao (incondicional). Refresher travado nao afeta a conexao — o ping
    segue e o front ve a lista velha (stale > desconectado)."""
    queue: asyncio.Queue = asyncio.Queue()
    cond = _list_refresher.acquire()

    async def reader():
        last_version = -1
        while True:
            async with cond:
                await cond.wait_for(lambda: _list_refresher.version != last_version)
                last_version = _list_refresher.version
                errored = _list_refresher.errored
                data = _list_refresher.data
            if errored:
                await queue.put(("list_error", "{}"))   # falha do refresher — front distingue de offline
            elif data is not None:
                await queue.put(("sessions", data))

    async def ping_loop():
        while True:
            await asyncio.sleep(ping_secs)
            await queue.put(("ping", "{}"))

    tasks = [asyncio.create_task(reader()), asyncio.create_task(ping_loop())]
    try:
        while True:
            event, data = await queue.get()
            yield {"event": event, "data": data}
    finally:
        for t in tasks:
            t.cancel()
        _list_refresher.release()


async def merged_events(name: str, jsonl: str, provider: str = "claude",
                        start_offset: int | None = None):
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
    # fica IGUAL pras duas fontes. Pi tambem e pane -> mesmo PreviewBroker, mas o provider VAI
    # JUNTO: o chrome que fecha o bloco em voo e outro (caixa do composer), ver preview.py.
    broker = CodexPreviewSource.get(name) if provider == "codex" else PreviewBroker.get(name, provider)
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
        return preview_is_committed(text, committed["text"])

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

    async def tail_pump(path: str, start_offset: int | None = None):
        # Transcript do .jsonl (msgs canonicas). Alem de emitir, RASTREIA a ultima msg de assistente
        # em `committed` -> fonte de verdade pra suprimir preview duplicado. E quando um bloco commita
        # que e exatamente o que o preview mostra, LIMPA o preview na hora (sem esperar o broker mudar).
        # Recebe o path (em vez de fechar sobre um tailer fixo) pra poder ser recriado no rebind do /clear.
        try:
            async for ev in adapter.transcript_stream(path, start_offset):
                if ev.kind == "assistant_msg" and ev.text:
                    committed["text"] = _norm(ev.text)
                    if _already_committed(preview_slot["text"]):
                        _enqueue_preview("")
                # 3o item da tupla = `id:` do SSE (None nos demais eventos). So o transcript ganha
                # id: e o unico stream com posicao retomavel. state/preview/ping NAO podem ter id --
                # o browser guarda o ULTIMO id visto, entao um ping carimbado sobrescreveria a
                # posicao real do transcript e a retomada pularia mensagens.
                #
                # O id carrega o STEM do jsonl junto com o offset ("<uuid>:<byte>"). Offset puro era
                # inseguro: apos um /clear o transcript e OUTRO arquivo, e um offset antigo que por
                # acaso coubesse no tamanho do novo passava na validacao, dava seek no meio dele e
                # PULAVA calado todo o inicio da conversa nova (parse_line engole a linha parcial).
                # Com o stem, id de outro transcript simplesmente nao e honrado.
                ev_id = f"{Path(path).stem}:{ev.offset}" if ev.offset is not None else None
                await queue.put(("message", ev.model_dump_json(), ev_id))
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

    # start_offset so vale pro tail INICIAL (veio do Last-Event-ID desta conexao). O rebind do
    # /clear abaixo recria sem ele: o transcript e outro arquivo, o offset antigo nao significa nada.
    tail_task = asyncio.create_task(tail_pump(jsonl, start_offset))
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
    # NUCLEO (conexao): instrumentacao do CICLO DE VIDA do stream. O sintoma relatado é "a conversa
    # para e só volta fechando/abrindo o app", e o log de acesso do uvicorn só mostra a conexão
    # FECHANDO — sem duração, sem motivo, sem quanto foi entregue. Sem isso a causa (queda de rede
    # do celular / iOS suspendendo / erro num pump / cancelamento) é indistinguível e vira chute.
    _t0 = time.monotonic()
    _sent = {"message": 0, "state": 0, "preview": 0, "ping": 0, "other": 0}
    _why = "cliente desconectou"
    _last_ctx_warn = {"sl": None}
    _log.info("sse: abriu name=%s provider=%s jsonl=%s", name, provider, Path(jsonl).name if jsonl else None)
    try:
        while True:
            # Só o tail_pump enfileira o 3o item (o offset -> `id:` do SSE); os demais produtores
            # continuam mandando pares. Desempacota tolerante em vez de tocar em todos eles.
            item = await queue.get()
            event, data = item[0], item[1]
            ev_id = item[2] if len(item) > 2 else None
            if event == "__error__":
                _why = f"erro no pump: {type(data).__name__}: {data}"
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
                # Diagnostico do "medição indisponível": loga o statusline CRU quando o segmento 💬
                # nao tem os 2 pares. Uma vez por statusline DISTINTO (nao a cada tick) pra nao virar
                # firehose — um StateEvent sai a cada 0.75s.
                _sl = parsed_state.get("status_line")
                if _sl and context_pairs(_sl) < 2 and _sl != _last_ctx_warn["sl"]:
                    _last_ctx_warn["sl"] = _sl
                    _log.info("sse: sem métrica de contexto name=%s statusline=%r", name, _sl)
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
            _sent[event if event in _sent else "other"] += 1
            out = {"event": event, "data": data}
            if ev_id is not None:
                out["id"] = str(ev_id)
            yield out
    except asyncio.CancelledError:
        # Fechamento NORMAL: o cliente sumiu e o starlette cancela o gerador. Distinguir isso de
        # um erro é o ponto — os dois terminavam o stream do mesmo jeito silencioso.
        _why = "cancelado (cliente sumiu / servidor encerrando)"
        raise
    finally:
        _log.info(
            "sse: fechou name=%s dur=%.1fs motivo=%s enviados=msg:%d state:%d preview:%d ping:%d",
            name, time.monotonic() - _t0, _why,
            _sent["message"], _sent["state"], _sent["preview"], _sent["ping"],
        )
        # So cancela e retorna (NAO await): um pump preso num asyncio.to_thread (tmux) nao e
        # cancelavel -> aguardar o gather aqui travava o aclose() do gerador, segurava a conexao
        # meio-aberta e, em rajada de reconexao do mobile, ia acumulando ate exaurir o threadpool
        # (a /api/sessions travava). Os inotify saem no GC; melhor isso que travar o disconnect.
        for t in tasks:
            t.cancel()
