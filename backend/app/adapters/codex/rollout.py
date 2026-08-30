"""Parser do rollout JSONL do Codex CLI -> ChatEvent (o mesmo shape neutro que o Claude produz).
Traduz o envelope `{type, payload}` do Codex; regras confirmadas contra codex-cli 0.141.0
(fixture em tests/fixtures/codex/rollout_sample.jsonl)."""
import hashlib
import json
import re

from app.transcript import ChatEvent

# message.role que sao system prompt/instrucoes internas do Codex, nao chat do usuario.
_NON_CHAT_ROLES = {"developer", "system"}

# O Codex injeta, nos 1os response_item de toda thread, role:"user" cujo conteudo e contexto interno
# (nao chat do usuario). Formatos vistos:
#   - wrapper em tag: <environment_context>...</environment_context>, <user_instructions>..., etc.
#   - blocos do host: <recommended_plugins>... e <permissions instructions>... (este ultimo usa
#     ESPACO no nome da tag, portanto nao casava o antigo `\w+_instructions`);
#   - AGENTS.md do projeto: comeca com "# AGENTS.md instructions for <path>" (quando o cwd tem AGENTS.md;
#     o env-context vem concatenado no fim dessa mesma msg gigante -> por isso o filtro de tag sozinho
#     nao pegava e a bolha vazava). Conservador: so casa se o texto (apos strip) COMECA com o marcador,
#     pra nao descartar uma mensagem real que apenas mencione a tag/o nome.
_CONTEXT_WRAPPER_RE = re.compile(
    r"^(<(?:environment_context|recommended_plugins|[a-z][a-z_ ]*instructions)>|"
    r"# AGENTS\.md instructions for )")


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


# The `exec` command comes INSIDE JavaScript code, not as an argument:
#   const r = await tools.exec_command({cmd:"echo oi","workdir":"/tmp",...}); text(r.output);
# The key appears with and without quotes (both forms are in this machine's rollouts), and the
# value is a JS string literal — it can carry escaped quotes and backslashes (`$'1\\n2'`, `\"...\"`).
_CMD_RE = re.compile(r'\bcmd"?\s*:\s*"((?:[^"\\]|\\.)*)"')


def _command_from_code(code: str) -> str:
    """The command inside the code, or "" when it can't be extracted.

    "" is a legitimate and frequent answer: `tools.write_stdin(...)` has no `cmd` at all. The caller
    keeps the raw code in that case — an empty `command` would draw an EMPTY summary line, which is
    worse than showing the code that actually ran. Single quotes and template literals also land
    here; neither appears in this machine's rollouts, so the fallback is what covers them.
    """
    m = _CMD_RE.search(code or "")
    if not m:
        return ""
    raw = m.group(1)
    try:
        # The JS literal matches JSON on the escapes seen here (\" \\ \n \t \uXXXX). On an escape
        # only JS has (\' , \x41) json fails and the raw value serves — it is already readable.
        return json.loads(f'"{raw}"')
    except (json.JSONDecodeError, ValueError):
        return raw


# `apply_patch` is the other custom tool, and it is Codex's `Edit`. Its input is the patch itself,
# which names each file it touches on a `*** <verb> File: <path>` line.
_PATCH_FILE_RE = re.compile(r"^\*{3} (?:Add|Update|Delete) File: (.+)$", re.MULTILINE)


def _files_from_patch(code: str) -> list[str]:
    """Paths a patch touches, or [] when it doesn't look like one.

    Without this the whole diff would be the summary line: the front has no `apply_patch` case, so
    it falls back to the first known key — and `file_path` is exactly that key. Handing over the raw
    patch would trade "invisible" for "truncated garbage", which is not an improvement.
    """
    return _PATCH_FILE_RE.findall(code or "")


def _output_text(output) -> str | None:
    """Text of a tool output: a LIST of blocks or a raw string — both shapes appear in real
    rollouts, so handling only the list would leave half the results empty."""
    if output is None:
        return None
    if isinstance(output, str):
        return output
    if isinstance(output, list):
        # Same block reader the messages use, with the same type filter: a block of another type
        # entering here and NOT there would be a difference nobody wrote on purpose.
        return _blocks_text(output, "input_text")
    return str(output)


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

    if ptype == "custom_tool_call":
        # `exec`, the tool Codex uses the most. Unlike `function_call`: `input` is a STRING of
        # code, not JSON arguments.
        code = payload.get("input")
        code = code if isinstance(code, str) else ""
        tool_input = {"code": code}
        # Only when it really came out: an empty value would make the front draw an EMPTY summary
        # line, worse than showing the code (see summarizeToolInput on the front). Which field is
        # salient depends on the tool — `exec` runs a command, `apply_patch` touches files — and
        # both names are ones the front already knows how to summarize.
        command = _command_from_code(code)
        files = _files_from_patch(code)
        if command:
            tool_input["command"] = command
        elif files:
            tool_input["file_path"] = files
        return [ChatEvent(
            kind="tool_use", id=_event_id(obj),
            tool_name=payload.get("name"), tool_use_id=payload.get("call_id"),
            tool_input=tool_input,
        )]

    if ptype == "custom_tool_call_output":
        # Output here is a LIST of blocks, not a scalar like in function_call_output.
        return [ChatEvent(
            kind="tool_result", id=_event_id(obj),
            tool_use_id=payload.get("call_id"),
            result=_output_text(payload.get("output")),
        )]

    if ptype == "function_call_output":
        # Same conversion as custom_tool_call_output: this type ALSO arrives as a list of blocks
        # (seen on the `wait` tool), and the previous `str()` put Python's repr in the conversation
        # — `[{'type': 'input_text', 'text': '...'}]` instead of the output.
        return [ChatEvent(
            kind="tool_result", id=_event_id(obj),
            tool_use_id=payload.get("call_id"),
            result=_output_text(payload.get("output")),
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
