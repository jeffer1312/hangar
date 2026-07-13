"""Parser do rollout JSONL do Codex CLI -> ChatEvent (o mesmo shape neutro que o Claude produz).
Traduz o envelope `{type, payload}` do Codex; regras confirmadas contra codex-cli 0.141.0
(fixture em tests/fixtures/codex/rollout_sample.jsonl)."""
import hashlib
import json
import re

from app.transcript import ChatEvent

# message.role que sao system prompt/instrucoes internas do Codex, nao chat do usuario.
_NON_CHAT_ROLES = {"developer", "system"}

# O Codex injeta, no 1o response_item de toda thread, um role:"user" cujo unico conteudo e um
# wrapper de contexto interno (<environment_context>...</environment_context> ou variantes tipo
# <user_instructions>...</user_instructions>) -- nao e chat do usuario. Conservador: so casa se o
# texto (apos strip) COMECA com a tag, pra nao descartar uma mensagem real que so mencione a tag.
_CONTEXT_WRAPPER_RE = re.compile(r"^<(environment_context|\w+_instructions)>")


def _is_context_wrapper(text: str) -> bool:
    return bool(_CONTEXT_WRAPPER_RE.match(text.strip()))


def _event_id(obj: dict) -> str:
    # O rollout do Codex nao tem uuid por entrada (diferente do jsonl do Claude). Hash
    # deterministico da linha inteira -> id estavel entre re-leituras (o front dedup por id).
    return hashlib.sha1(json.dumps(obj, sort_keys=True, default=str).encode()).hexdigest()


def _blocks_text(content, block_type: str) -> str:
    """Concatena o texto dos blocos `block_type` de `content`. Aceita `content` como string OU
    lista de blocos; blocos de tipo desconhecido sao ignorados sem quebrar."""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "".join(
        block.get("text", "")
        for block in content
        if isinstance(block, dict) and block.get("type") == block_type
    )


def parse_rollout_obj(obj: dict) -> list[ChatEvent]:
    """Eventos de chat de UMA linha ja parseada do rollout. So `response_item` vira chat —
    session_meta/turn_context/world_state/compacted/event_msg sao estado, nao conversa."""
    if obj.get("type") != "response_item":
        return []
    payload = obj.get("payload")
    if not isinstance(payload, dict):
        return []
    ptype = payload.get("type")

    if ptype == "message":
        role = payload.get("role")
        if role in _NON_CHAT_ROLES:
            return []
        if role == "user":
            text = _blocks_text(payload.get("content"), "input_text")
            if _is_context_wrapper(text):
                return []
            return [ChatEvent(kind="user_msg", id=_event_id(obj), text=text)]
        if role == "assistant":
            text = _blocks_text(payload.get("content"), "output_text")
            return [ChatEvent(kind="assistant_msg", id=_event_id(obj), text=text)]
        return []

    if ptype == "function_call":
        try:
            tool_input = json.loads(payload.get("arguments") or "{}")
        except (json.JSONDecodeError, ValueError):
            tool_input = {}
        return [ChatEvent(
            kind="tool_use", id=_event_id(obj),
            tool_name=payload.get("name"), tool_use_id=payload.get("call_id"),
            tool_input=tool_input if isinstance(tool_input, dict) else {},
        )]

    if ptype == "function_call_output":
        output = payload.get("output")
        return [ChatEvent(
            kind="tool_result", id=_event_id(obj),
            tool_use_id=payload.get("call_id"),
            result=str(output) if output is not None else None,
        )]

    # reasoning: encrypted_content opaco no rollout -> ignora no v1 (texto legivel so ao vivo).
    return []


def parse_rollout_line(line: str) -> list[ChatEvent]:
    """Parseia uma linha crua do rollout .jsonl -> ChatEvent. Espelha transcript.parse_line."""
    line = line.strip()
    if not line:
        return []
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return []
    return parse_rollout_obj(obj)
