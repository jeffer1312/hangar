"""wire.jsonl do Kimi -> ChatEvent.

Parser proprio, irmao do de adapters/pi/transcript.py: o formato do Kimi nao e o do Claude (role no
topo) nem o do Pi (role dentro de `message`) — e um envelope `{"type":..., "time":<ms>}` com a
conversa espalhada por `context.append_message` (usuario) e `context.append_loop_event` (assistente,
tools). Medido no Kimi 0.34.0 em sessoes reais desta maquina:

  - usuario:  context.append_message, message.role=="user". FILTRO OBRIGATORIO
              message.origin.kind=="user": cada prompt gera um SEGUNDO append_message com
              origin.kind=="injection" (o system-reminder de permission mode) que, sem o filtro,
              vira bolha fantasma no chat.
  - texto:    loop event content.part, part.type=="text" ("think" e rascunho interno -> fora,
              igual ao thinking do Claude/Pi). Um part por step (medido: sem chunking).
  - tool_use: loop event tool.call {toolCallId, name, args}.
  - tool_result: loop event tool.result {toolCallId, result:{output}}.
  - ts:       envelope "time" em MILISSEGUNDOS -> ChatEvent.ts em segundos.
  - ignorar:  metadata, profile.bind, config.update, llm.*, turn.prompt (duplica o append_message),
              turn.steer, turn.ended, permission.set_mode, plan_mode.*, step.begin/end.

cache_read fica de fora de proposito: o usage.record e evento IRMAO dos parts (nao vem dentro
deles), entao associar exigiria estado entre linhas; o campo e Optional no ChatEvent e o front
vive sem ele. ponytail: se um dia importar, um Stream com memoria (como o do Pi) resolve.

`ChatEvent` e contrato com o front: aqui so muda a FONTE, nunca o shape.
"""
import json
import logging
from typing import Optional

from app.models import ChatEvent

_log = logging.getLogger("hangar.kimi")

# Limite da cauda lida procurando pergunta pendente (mesmo criterio do Pi: a pergunta e sempre
# recente — o agente inteiro para esperando ela).
_PENDQ_TAIL_BYTES = 512 * 1024

_ASKQ = "AskUserQuestion"


def _ts(obj: dict) -> Optional[float]:
    t = obj.get("time")
    return t / 1000.0 if isinstance(t, (int, float)) else None


def _loop_event(obj: dict) -> Optional[dict]:
    if obj.get("type") != "context.append_loop_event":
        return None
    ev = obj.get("event")
    return ev if isinstance(ev, dict) else None


def _result_text(result: object) -> str:
    # Medido: result e um objeto {"output": "..."}. Se um dia vier string solta, passa direto.
    if isinstance(result, dict):
        out = result.get("output")
        return out if isinstance(out, str) else json.dumps(result, ensure_ascii=False)
    return result if isinstance(result, str) else ""


def parse_obj(obj: dict) -> list[ChatEvent]:
    t = obj.get("type")

    if t == "context.append_message":
        msg = obj.get("message")
        if not isinstance(msg, dict):
            return []
        if msg.get("role") != "user":
            return []
        origin = msg.get("origin")
        if not isinstance(origin, dict) or origin.get("kind") != "user":
            return []  # injection/steer: contexto do sistema, nao bolha (ver docstring)
        out: list[ChatEvent] = []
        content = msg.get("content")
        if not isinstance(content, list):
            return []
        base_id = msg.get("id") or ""
        for k, b in enumerate(content):
            if isinstance(b, dict) and b.get("type") == "text" and (b.get("text") or "").strip():
                # Uma mensagem com N blocos vira N eventos; o sufixo evita colisao de id (front
                # deduplica por id) — mesmo truque do _sub_id do Pi.
                eid = base_id if k == 0 else f"{base_id}:{k}"
                out.append(ChatEvent(kind="user_msg", id=eid, text=b["text"], ts=_ts(obj)))
        return out

    ev = _loop_event(obj)
    if ev is None:
        return []
    et = ev.get("type")
    eid = ev.get("uuid") or ""

    if et == "content.part":
        part = ev.get("part")
        if not isinstance(part, dict):
            return []
        if part.get("type") == "text" and (part.get("text") or "").strip():
            return [ChatEvent(kind="assistant_msg", id=eid, text=part["text"], ts=_ts(obj))]
        return []

    if et == "tool.call":
        args = ev.get("args")
        return [ChatEvent(kind="tool_use", id=eid, tool_name=ev.get("name"),
                          tool_input=args if isinstance(args, dict) else {},
                          tool_use_id=ev.get("toolCallId"), ts=_ts(obj))]

    if et == "tool.result":
        result = ev.get("result")
        is_error = result.get("isError") if isinstance(result, dict) else None
        # O `tool.result` do Kimi NAO tem `uuid` — so `parentUuid` (o uuid do tool.call) e
        # `toolCallId`. Medido em 14/08/2026 numa sessao real: 205 dos 205 tool_result saiam com
        # id="" e o app deduplica eventos POR ID (Chat.svelte, `idIndex`) — logo TODOS ocupavam o
        # mesmo slot: cada resultado novo SUBSTITUIA o anterior, e a sessao inteira ficava com um
        # resultado so. Dois estragos visiveis: todo card de ferramenta preso em "Executando…", e a
        # pergunta do AskUserQuestion REABRINDO depois de respondida (a lista deriva "respondida" da
        # presenca do tool_result; quando a ferramenta seguinte tomava o slot, a pergunta voltava a
        # parecer pendente). Prefixo "res:" pra nunca colidir com o uuid do proprio tool.call.
        cid = ev.get("toolCallId") or ev.get("parentUuid") or eid
        if not cid:
            # Terceiro elo vazio = o Kimi mudou o formato. Volta o bug de cima (todos os resultados
            # no mesmo slot), e sem esta linha ele voltaria CALADO, do mesmo jeito que apareceu.
            _log.warning("kimi: tool.result sem toolCallId/parentUuid/uuid — id vazio: %.200s", ev)
        return [ChatEvent(kind="tool_result", id=f"res:{cid}" if cid else "",
                          tool_use_id=ev.get("toolCallId"),
                          result=_result_text(result), is_error=bool(is_error), ts=_ts(obj))]

    return []


def parse_line(line: str) -> list[ChatEvent]:
    line = line.strip()
    if not line:
        return []
    try:
        obj = json.loads(line)
    except ValueError:
        # O tailer le enquanto o Kimi escreve: linha pela metade e normal, nao e erro.
        return []
    if not isinstance(obj, dict):
        return []
    return parse_obj(obj)


# ── Pergunta nativa do Kimi (tool AskUserQuestion) ───────────────────────────
# Mesmo raciocinio do Pi: o tool.call cai no wire com os args COMPLETOS no instante da pergunta e o
# tool.result so chega depois da resposta. Pendente = tool.call AskUserQuestion sem tool.result com
# o mesmo toolCallId DEPOIS dele. (O Kimi tambem tem o hook PermissionRequest, mas ele nao carrega
# as opcoes da pergunta — so o wire tem.)


def read_pending_question(jsonl: str) -> dict | None:
    """Os `args` do ultimo tool.call AskUserQuestion ainda sem resposta, ou None."""
    pend = read_pending_call(jsonl)
    return pend[1] if pend else None


def resposta_chegou(jsonl: str, call_id: str) -> bool:
    """True SO quando o `tool.result` daquele `toolCallId` esta comprovadamente no wire.

    E a PROVA de entrega do drive do picker, e vale mais que reler o pane: o Kimi so escreve este
    evento depois de a ferramenta receber as respostas de verdade. Medido em 13/08/2026 (Kimi
    0.36.0): sem passar pela tela de Review, o result nunca aparece.

    Procura o result DIRETO, em vez de deduzir pela ausencia da pergunta pendente. A deducao parecia
    equivalente e nao era: `read_pending_call` devolve None tanto pra "respondida" quanto pra
    "arquivo sumiu" e "linha corrompida" — e ai um wire ilegivel virava "entregue com sucesso", que e
    exatamente a confirmacao por ausencia de dado que esta funcao existe pra impedir. Nao deu pra
    ler = nao chegou; quem chama tenta de novo ate o prazo e cai no fallback por texto."""
    objs = _objetos_da_cauda(jsonl, '"context.append_loop_event"')
    if objs is None:
        return False
    for obj in objs:
        ev = _loop_event(obj)
        if ev and ev.get("type") == "tool.result" and ev.get("toolCallId") == call_id:
            return True
    return False


_CORTE_AVISADO: set[str] = set()


def _objetos_da_cauda(jsonl: str, marca: str, avisa_corte: bool = False) -> list[dict] | None:
    """Objetos JSON da CAUDA do wire cuja linha CRUA contem `marca`; None = nao deu pra ler.

    None e ausencia de dado, nunca "nada encontrado". Sao respostas diferentes pro mesmo erro: quem
    procura pendencia devolve None (e o /answer vira 409 legivel), quem PROVA entrega devolve False
    (e o caller tenta de novo). Fundir as duas foi o furo que a docstring de `resposta_chegou`
    descreve — um wire ilegivel virando "entregue com sucesso".

    `marca` e so filtro barato antes do json.loads; quem decide e sempre o campo `type` do objeto.
    Linha corrompida cai fora sozinha: o tailer le enquanto o Kimi escreve."""
    try:
        with open(jsonl, "rb") as f:
            f.seek(0, 2)
            size = f.tell()
            f.seek(max(0, size - _PENDQ_TAIL_BYTES))
            tail = f.read().decode("utf-8", errors="replace")
    except OSError:
        return None
    linhas = tail.splitlines()
    if size > _PENDQ_TAIL_BYTES:
        # A primeira veio cortada pelo seek no meio e nao da pra parsear. Quase sempre e lixo — o
        # Kimi grava linhas enormes (o `config.update` leva o system prompt inteiro; ha `tool.result`
        # de 1,9MB medidos nesta maquina) —, e e por isso que o aviso e OPCIONAL: com a marca do
        # envelope de loop event ele dispararia a cada poll, sem querer dizer nada.
        #
        # Com a marca de `interaction.` e diferente: ali um corte significa que o pedido de
        # aprovacao PENDENTE nao foi lido, e a funcao devolveria "nada encontrado" — calada, com os
        # botoes sumindo da tela sem explicacao. Acontece quando a propria linha do pedido passa da
        # janela: um `interaction.request` de plano carrega o markdown inteiro em `display.plan`, e
        # hoje o maior medido tem 14KB contra os 512KB daqui. Nao ampliamos a janela por um caso que
        # ainda nao existe (custo por poll) — o que nao pode e ele acontecer sem rastro. Uma linha
        # por arquivo, mesma disciplina do `_avisa_uma_vez` do state.py: quem chama e o poll.
        if avisa_corte and marca in linhas[0] and jsonl not in _CORTE_AVISADO:
            _CORTE_AVISADO.add(jsonl)
            _log.warning("kimi: a cauda de %dKB cortou uma linha %s em %s — o pedido pendente pode "
                         "nao ser lido", _PENDQ_TAIL_BYTES // 1024, marca, jsonl)
        linhas = linhas[1:]
    objs: list[dict] = []
    for linha in linhas:
        linha = linha.strip()
        if not linha or marca not in linha:
            continue
        try:
            obj = json.loads(linha)
        except ValueError:
            continue
        if isinstance(obj, dict):
            objs.append(obj)
    return objs


def read_pending_call(jsonl: str) -> tuple[str, dict] | None:
    """(toolCallId, args) do ultimo tool.call AskUserQuestion ainda sem resposta, ou None.

    O id vai junto de proposito: e por ele que o drive do picker confirma a entrega (o `tool.result`
    do MESMO id aparecendo no wire), em vez de reler a tela.

    Le so a CAUDA do wire. Malformado/ausente -> None, nunca levanta: quem chama e o /answer, e um
    None vira 409 legivel pro usuario."""
    # Filtro rapido pelo ENVELOPE, nao pelo nome da tool: o tool.result do Kimi NAO carrega o nome
    # (so o toolCallId) — filtrar por "AskUserQuestion" descartaria a RESPOSTA e a pergunta nunca
    # sairia de pendente (bug pego pelo teste).
    objs = _objetos_da_cauda(jsonl, '"context.append_loop_event"')
    if objs is None:
        return None
    last_q: tuple[str, dict] | None = None
    answered: set[str] = set()
    for obj in objs:
        ev = _loop_event(obj)
        if ev is None:
            continue
        if ev.get("type") == "tool.result":
            cid = ev.get("toolCallId")
            if cid:
                answered.add(cid)
            continue
        if (ev.get("type") == "tool.call" and ev.get("name") == _ASKQ
                and isinstance(ev.get("args"), dict) and ev.get("toolCallId")):
            last_q = (ev["toolCallId"], ev["args"])
    if last_q is None or last_q[0] in answered:
        return None
    return last_q


# ── Pedido de APROVACAO do Kimi (plano, comando, arquivo) ────────────────────
# O painel de aprovacao — "Ready to build with this plan? 1. Approve 2. Reject 3. Revise" — nao
# passava NADA pro app: o `_CURSOR_RE`/`_OPTION_RE` do state.py so conhecem `❯` (Claude) e `>` (Pi),
# e o Kimi desenha `▶`. Ensinar o glifo aos regexes seria o remendo errado, e foi medido frouxo (a
# 3a opcao voltava grudada no rascunho do composer). O Kimi nao precisa de raspagem nenhuma: ele
# grava `interaction.request` no wire com tudo que o painel usa pra se desenhar.
#
# O que o wire NAO traz sao os ROTULOS das opcoes — eles nascem no proprio TUI, em
# `src/tui/reverse-rpc/approval/adapter.ts` (`adaptChoices`), a partir de `toolName` e `display`.
# As tabelas abaixo sao esse `adaptChoices` transcrito, lido do binario do Kimi 0.34.0, e sao um
# CALIBRATION KNOB: se um dia o Kimi renomear uma escolha, e aqui que se ajusta. Errar o rotulo NAO
# manda tecla errada — quem escolhe e a POSICAO, que e o que o painel le da tecla numerica —, o
# usuario e que leria um nome desatualizado no botao.
#
# `requires_feedback` (Revise / Reject with feedback) muda o que acontece DEPOIS da tecla: em vez de
# submeter, o painel abre um campo de texto e espera a justificativa. Quem dirige precisa saber
# disso pra nao ficar esperando um `interaction.resolved` que so vem depois de a pessoa escrever.

# `DEFAULT_APPROVAL_CHOICES` do TUI: vale pra todo pedido que nao e plano nem inicio de goal.
_APROV_PADRAO: tuple[tuple[str, bool], ...] = (
    ("Approve once", False),
    ("Approve for this session", False),
    ("Reject", False),
    ("Reject with feedback", True),
)
# `PLAN_REJECT_CHOICES`: vao SEMPRE no fim das escolhas de plano, nesta ordem.
_APROV_PLANO_FIM: tuple[tuple[str, bool], ...] = (("Reject", False), ("Revise", True))
# `goalStartOptions(display.mode)`.
_APROV_GOAL: dict[str, tuple[tuple[str, bool], ...]] = {
    "yolo": (("Switch to Auto and start", False), ("Keep YOLO and start", False),
             ("Do not start", False)),
    "manual": (("Switch to Auto and start", False), ("Switch to YOLO and start", False),
               ("Start in Manual", False), ("Do not start", False)),
}
# `headerFor(tool_name)`: o titulo que o painel escreve em cima das opcoes.
_APROV_TITULO = {
    "Bash": "Run this command?",
    "Write": "Write this file?",
    "Edit": "Apply these edits?",
    "TaskStop": "Stop this task?",
    "ExitPlanMode": "Ready to build with this plan?",
}


def _escolhas_de_aprovacao(tool_name: str, display: dict) -> list[tuple[str, bool]]:
    """As escolhas do painel, na ORDEM em que ele as numera. Transcricao do `adaptChoices` do TUI."""
    kind = display.get("kind")
    if tool_name == "ExitPlanMode" or kind == "plan_review":
        opcoes = display.get("options")
        # `>= 2` e do TUI, nao arredondamento nosso: com uma opcao so ele ignora a lista e desenha
        # o "Approve" generico.
        if isinstance(opcoes, list) and len(opcoes) >= 2:
            proprias = [(str(o.get("label") or ""), False)
                        for o in opcoes if isinstance(o, dict) and o.get("label")]
        else:
            proprias = [("Approve", False)]
        return proprias + list(_APROV_PLANO_FIM)
    if kind == "goal_start":
        modo = display.get("mode")
        return list(_APROV_GOAL["yolo" if modo == "yolo" else "manual"])
    return list(_APROV_PADRAO)


def _resumo_de_aprovacao(display: dict, acao: str) -> str:
    """A linha que o app mostra acima dos botoes. Crase vira `<code>` no front (OptionButtons)."""
    kind = display.get("kind")
    if kind == "command" and display.get("command"):
        return f"`{display['command']}`"
    if kind in ("file_io", "diff") and display.get("path"):
        return f"`{display['path']}`"
    if kind == "url_fetch" and display.get("url"):
        return f"`{display['url']}`"
    if kind == "search" and display.get("query"):
        return f"`{display['query']}`"
    # plan_review nao ganha resumo de proposito: o plano inteiro ja esta no chat, como o tool_use do
    # ExitPlanMode. Repetir um pedaco dele aqui so empurraria os botoes pra fora da tela.
    return "" if kind == "plan_review" else (acao or "")


def read_pending_interaction(jsonl: str) -> dict | None:
    """O pedido de APROVACAO pendente no fim do wire, ou None.

    Pendente = `interaction.request` de `kind: "approval"` sem `interaction.resolved` de mesmo `id`.

    So aprovacao: `kind: "question"` e o AskUserQuestion, que ja chega ao app pelo tool.call
    (`read_pending_call`) e abre o stepper nativo. Emitir os dois faria a mesma pergunta aparecer
    como stepper E como fila de botoes ao mesmo tempo.

    Devolve `{"id", "tool_name", "kind_display", "titulo", "resumo", "escolhas"}`, com `escolhas`
    sendo `[{"label", "requires_feedback"}]` na ordem que o painel numera."""
    objs = _objetos_da_cauda(jsonl, '"interaction.', avisa_corte=True)
    if objs is None:
        return None
    pendente: tuple[str, dict] | None = None
    resolvidas: set[str] = set()
    for obj in objs:
        t = obj.get("type")
        if t == "interaction.resolved":
            rid = obj.get("id")
            if isinstance(rid, str):
                resolvidas.add(rid)
        elif t == "interaction.request" and obj.get("kind") == "approval":
            rid, req = obj.get("id"), obj.get("request")
            if isinstance(rid, str) and isinstance(req, dict):
                pendente = (rid, req)
    if pendente is None or pendente[0] in resolvidas:
        return None
    rid, req = pendente
    display = req.get("display")
    if not isinstance(display, dict):
        # Sem display nao da pra saber quais escolhas o painel desenhou, e chutar as 4 do padrao
        # mandaria a tecla `2` pra uma opcao que talvez nem exista. Melhor nao oferecer botao —
        # a pessoa responde no terminal, como antes deste caminho existir.
        _log.warning("kimi: interaction.request sem display id=%s — sem botoes no app", rid)
        return None
    tool_name = str(req.get("toolName") or "")
    escolhas = _escolhas_de_aprovacao(tool_name, display)
    if not escolhas:
        return None
    return {
        "id": rid,
        "tool_name": tool_name,
        "kind_display": display.get("kind"),
        "titulo": _APROV_TITULO.get(tool_name, f"Approve {tool_name}?" if tool_name
                                    else "Approve action?"),
        "resumo": _resumo_de_aprovacao(display, str(req.get("action") or "")),
        "escolhas": [{"label": rot, "requires_feedback": fb} for rot, fb in escolhas],
    }


def interacao_resolvida(jsonl: str, req_id: str) -> bool:
    """True SO quando o `interaction.resolved` daquele pedido esta comprovadamente no wire.

    Mesma disciplina do `resposta_chegou`: e a PROVA de que o painel recebeu a escolha. Wire
    ilegivel devolve False — nao deu pra ler nao e "chegou"."""
    objs = _objetos_da_cauda(jsonl, '"interaction.resolved"')
    if objs is None:
        return False
    return any(o.get("type") == "interaction.resolved" and o.get("id") == req_id for o in objs)
