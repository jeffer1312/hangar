"""JSONL do Pi -> ChatEvent.

Parser proprio, irmao do de app/transcript.py e nao um ramo dentro dele: o formato do Pi poe o
role DENTRO de `message` (o Claude poe no topo) e da ao resultado de tool um role proprio
(`toolResult`) em vez de enfiar o bloco numa mensagem de usuario. Misturar os dois num parser so
transformaria cada mudanca de formato de um provider em risco de regressao no outro.

`ChatEvent` e contrato com o front: aqui so muda a FONTE, nunca o shape.
"""
import json
import logging
import re
from typing import Optional

from app.models import ChatEvent, scrub_surrogates

_log = logging.getLogger("claude_pocket.pi_transcript")

# Blocos que nao viram bolha de chat. `thinking` fica de fora igual no Claude: e rascunho interno,
# nao resposta.
_DROPPED_BLOCK_TYPES = {"thinking"}

# O Pi grava o texto JA COLORIDO no JSONL: toda resposta final termina com
# "\x1b[38;2;136;136;136m✻ Turn took 2s\x1b[0m", e alguns resultados de tool tambem vem com cor.
# A bolha do app nao interpreta terminal — sem tirar, o usuario le "[38;2;136;136;136m✻ Turn took".
# Pega a sequencia CSI inteira (SGR e vizinhas), nao so o `m`.
_ANSI_RE = re.compile(r"\x1b\[[0-9;:?]*[ -/]*[@-~]")


def _clean(text: str) -> str:
    # scrub_surrogates aqui e nao so na rede do ChatEvent porque ESTE e o ponto onde o texto do Pi
    # entra normalizado: o Pi trunca por unidade UTF-16 e parte emoji ao meio ("...\ud83d...
    # [truncated]" num toolResult), sobrando um surrogate solto que nao da pra serializar em JSON.
    return scrub_surrogates(_ANSI_RE.sub("", text))


def _sub_id(node_id: str, k: int) -> str:
    # Uma mensagem com N blocos vira N eventos; sem sufixo eles colidiriam e o front deduplicaria.
    return node_id if k == 0 else f"{node_id}:{k}"


def _result_text(content: list) -> str:
    parts: list[str] = []
    for b in content:
        if not isinstance(b, dict):
            continue
        if b.get("type") == "text":
            parts.append(_clean(b.get("text") or ""))
        elif b.get("type") == "image":
            # `data` e base64 inteiro: inline mandaria megabytes pelo SSE a cada evento.
            parts.append(f"[imagem {b.get('mimeType') or 'desconhecida'}]")
    return "\n".join(p for p in parts if p)


def parse_obj(obj: dict) -> list[ChatEvent]:
    if obj.get("type") != "message":
        return []      # session / model_change / thinking_level_change nao sao conversa
    msg = obj.get("message")
    if not isinstance(msg, dict):
        return []
    node_id = obj.get("id") or ""
    ts: Optional[float] = None
    raw_ts = msg.get("timestamp")
    if isinstance(raw_ts, (int, float)):
        ts = raw_ts / 1000.0     # Pi grava epoch em MILISSEGUNDOS; ChatEvent.ts e em segundos
    role = msg.get("role")
    content = msg.get("content")
    if not isinstance(content, list):
        return []

    if role == "toolResult":
        # tool_name fica de fora: nem o parser do Claude nem o do Codex o preenchem em tool_result
        # (o front pega o nome do tool_use casado por tool_use_id, ver MessageList.svelte:99).
        return [ChatEvent(kind="tool_result", id=node_id,
                          tool_use_id=msg.get("toolCallId"),
                          result=_result_text(content),
                          is_error=bool(msg.get("isError")), ts=ts)]

    if role == "user":
        out: list[ChatEvent] = []
        for k, b in enumerate(content):
            if isinstance(b, dict) and b.get("type") == "text" and (b.get("text") or "").strip():
                out.append(ChatEvent(kind="user_msg", id=_sub_id(node_id, k),
                                     text=_clean(b["text"]), ts=ts))
        return out

    if role == "assistant":
        usage = msg.get("usage") if isinstance(msg.get("usage"), dict) else {}
        cache_read = usage.get("cacheRead") if isinstance(usage.get("cacheRead"), int) else None
        out = []
        for k, b in enumerate(content):
            if not isinstance(b, dict):
                continue
            t = b.get("type")
            if t in _DROPPED_BLOCK_TYPES:
                continue
            if t == "text" and (b.get("text") or "").strip():
                out.append(ChatEvent(kind="assistant_msg", id=_sub_id(node_id, k),
                                     text=_clean(b["text"]), cache_read=cache_read, ts=ts))
            elif t == "toolCall":
                args = b.get("arguments")
                out.append(ChatEvent(kind="tool_use", id=_sub_id(node_id, k),
                                     tool_name=b.get("name"),
                                     tool_input=args if isinstance(args, dict) else {},
                                     tool_use_id=b.get("id"), ts=ts))
        return out

    return []


def parse_line(line: str) -> list[ChatEvent]:
    line = line.strip()
    if not line:
        return []
    try:
        obj = json.loads(line)
    except ValueError:
        # O tailer le enquanto o Pi escreve: linha pela metade e normal, nao e erro.
        return []
    if not isinstance(obj, dict):
        return []
    return parse_obj(obj)


# Contexto que um hook do Claude injetou no turno, via a extensao claude-hooks-adapter.ts (ela roda
# os hooks do Claude dentro do Pi e devolve o texto pelo `before_agent_start`). O Pi COLA esse texto
# no inicio da mensagem do usuario — e essa mensagem e o que o app mostra como bolha, entao o
# "[skill-suggester] ..." aparecia como se o usuario tivesse digitado. No terminal nao aparece.
# O corte e so de EXIBICAO: o arquivo nao e reescrito e o modelo segue recebendo o contexto.
_HOOK_CTX_TYPE = "claude-hook-context"


class Stream:
    """Parser com UMA linha de memoria: segura o user_msg ate ver a linha seguinte.

    Existe porque o sinal que identifica o texto do hook chega DEPOIS da mensagem: o Pi grava a
    mensagem do usuario (com o contexto colado) e so na linha de baixo um
    `custom_message/claude-hook-context` com o mesmo texto e `parentId` apontando pra ela. Casar por
    esse par e exato; casar por padrao de texto ("[algo] ...") seria chute e um dia comeria mensagem
    de verdade. A espera custa nada na pratica: as duas linhas nascem no mesmo milissegundo e o
    tailer le as duas na mesma passada. Nao chegou a irmã (sessao sem a extensao de hooks) -> o
    flush solta a mensagem intacta no fim do lote.

    Cada tail/leitura de historico cria a sua — estado de modulo seria compartilhado entre sessoes.
    """

    def __init__(self) -> None:
        self._held: list[tuple[float, ChatEvent]] = []
        self._held_id = ""

    def tem_retido(self) -> bool:
        """Ha mensagem presa esperando a irmã? O tailer usa isto pra so pagar a espera curta
        quando ela tem pra que servir (ver TranscriptTailer._flush_com_espera)."""
        return bool(self._held)

    def _release(self) -> list[tuple[float, ChatEvent]]:
        if self._held and self._held_id:
            # Soltou sem par: ou a sessao nao tem a extensao de hooks (o caso comum, silencioso de
            # proposito) ou a irmã se perdeu e o prefixo do hook vai vazar pra bolha. Nao da pra
            # distinguir os dois aqui; o debug e o unico rastro se o vazamento voltar.
            _log.debug("pi: user_msg %s solto sem o marcador de hook", self._held_id)
        out, self._held, self._held_id = self._held, [], ""
        return out

    def feed(self, obj: dict, ts: float = 0.0) -> list[tuple[float, ChatEvent]]:
        if (obj.get("type") == "custom_message" and obj.get("customType") == _HOOK_CTX_TYPE
                and self._held and obj.get("parentId") == self._held_id):
            ctx = _clean(obj.get("content") or "").strip()
            if ctx:
                self._held = [(t, ev) for t, ev in self._held if _strip_prefix(ev, ctx)]
            return self._release()
        out = self._release()
        evs = parse_obj(obj)
        node_id = obj.get("id") or ""
        if node_id and evs and all(ev.kind == "user_msg" for ev in evs):
            self._held = [(ts, ev) for ev in evs]
            self._held_id = node_id
            return out
        return out + [(ts, ev) for ev in evs]

    def flush(self) -> list[tuple[float, ChatEvent]]:
        return self._release()

    def parse_line(self, line: str) -> list[ChatEvent]:
        """Forma que o TranscriptTailer espera (linha -> eventos), sem o ts."""
        line = line.strip()
        if not line:
            return []
        try:
            obj = json.loads(line)
        except ValueError:
            return []
        if not isinstance(obj, dict):
            return []
        return [ev for _, ev in self.feed(obj)]

    def flush_events(self) -> list[ChatEvent]:
        return [ev for _, ev in self.flush()]

    def feed_events(self, obj: dict) -> list[ChatEvent]:
        """Forma que o merged_history espera (objeto -> eventos). O ts de cada evento ja vem do
        parser (message.timestamp), entao o caller nao perde a ordem por soltar um evento retido
        junto com a linha seguinte."""
        return [ev for _, ev in self.feed(obj)]


def _strip_prefix(ev: ChatEvent, ctx: str) -> bool:
    """Tira o contexto do hook do inicio da bolha. False = nao sobrou nada, o evento inteiro sai."""
    text = (ev.text or "").strip()
    if not text.startswith(ctx):
        return True                      # bloco seguinte da mesma mensagem: nada a cortar
    ev.text = text[len(ctx):].strip()
    return bool(ev.text)
