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
from typing import Optional

from app.models import ChatEvent

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
    try:
        with open(jsonl, "rb") as f:
            f.seek(0, 2)
            size = f.tell()
            f.seek(max(0, size - _PENDQ_TAIL_BYTES))
            tail = f.read().decode("utf-8", errors="replace")
    except OSError:
        return False
    lines = tail.splitlines()
    if size > _PENDQ_TAIL_BYTES:
        lines = lines[1:]                  # primeira linha veio cortada pelo seek no meio
    for line in lines:
        line = line.strip()
        if not line or '"context.append_loop_event"' not in line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        ev = _loop_event(obj)
        if ev and ev.get("type") == "tool.result" and ev.get("toolCallId") == call_id:
            return True
    return False


def read_pending_call(jsonl: str) -> tuple[str, dict] | None:
    """(toolCallId, args) do ultimo tool.call AskUserQuestion ainda sem resposta, ou None.

    O id vai junto de proposito: e por ele que o drive do picker confirma a entrega (o `tool.result`
    do MESMO id aparecendo no wire), em vez de reler a tela.

    Le so a CAUDA do wire. Malformado/ausente -> None, nunca levanta: quem chama e o /answer, e um
    None vira 409 legivel pro usuario."""
    try:
        with open(jsonl, "rb") as f:
            f.seek(0, 2)
            size = f.tell()
            f.seek(max(0, size - _PENDQ_TAIL_BYTES))
            tail = f.read().decode("utf-8", errors="replace")
    except OSError:
        return None
    lines = tail.splitlines()
    if size > _PENDQ_TAIL_BYTES:
        lines = lines[1:]  # primeira linha veio cortada pelo seek no meio
    last_q: tuple[str, dict] | None = None
    answered: set[str] = set()
    for line in lines:
        line = line.strip()
        # Filtro rapido pelo ENVELOPE, nao pelo nome da tool: o tool.result do Kimi NAO carrega o
        # nome (so o toolCallId) — filtrar por "AskUserQuestion" descartaria a RESPOSTA e a
        # pergunta nunca sairia de pendente (bug pego pelo teste).
        if not line or '"context.append_loop_event"' not in line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
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
