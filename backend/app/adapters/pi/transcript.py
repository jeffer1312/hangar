"""JSONL do Pi -> ChatEvent.

Parser proprio, irmao do de app/transcript.py e nao um ramo dentro dele: o formato do Pi poe o
role DENTRO de `message` (o Claude poe no topo) e da ao resultado de tool um role proprio
(`toolResult`) em vez de enfiar o bloco numa mensagem de usuario. Misturar os dois num parser so
transformaria cada mudanca de formato de um provider em risco de regressao no outro.

`ChatEvent` e contrato com o front: aqui so muda a FONTE, nunca o shape.
"""
import json
import re
from typing import Optional

from app.models import ChatEvent

# Blocos que nao viram bolha de chat. `thinking` fica de fora igual no Claude: e rascunho interno,
# nao resposta.
_DROPPED_BLOCK_TYPES = {"thinking"}

# O Pi grava o texto JA COLORIDO no JSONL: toda resposta final termina com
# "\x1b[38;2;136;136;136m✻ Turn took 2s\x1b[0m", e alguns resultados de tool tambem vem com cor.
# A bolha do app nao interpreta terminal — sem tirar, o usuario le "[38;2;136;136;136m✻ Turn took".
# Pega a sequencia CSI inteira (SGR e vizinhas), nao so o `m`.
_ANSI_RE = re.compile(r"\x1b\[[0-9;:?]*[ -/]*[@-~]")


def _clean(text: str) -> str:
    return _ANSI_RE.sub("", text)


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
