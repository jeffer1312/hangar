import asyncio
import base64
import json
import re
from pathlib import Path
from typing import AsyncIterator, Optional
from watchfiles import awatch
from app.models import ChatEvent

# Imagem colada no TERMINAL (TUI do Claude). O Claude grava 2 coisas: a msg do user com um bloco
# `image` (base64) + um marcador "[Image #N]" no texto; E uma entrada user SINTETICA cujo texto é só
# "[Image: source: <path>]" (referência). A 1ª vira bubble com thumbnail (image_count); a 2ª é meta.
# Quando o MODELO le uma imagem (tool Read), o harness injeta outra entrada user sintetica cujo texto
# e so "[Image: original WxH, displayed at ...]" (ou "[Image]" sem resize) — tambem meta, nao conversa.
# Pega qualquer entrada cujo texto INTEIRO seja "[Image]" ou "[Image: ...]": usuario nunca digita isso.
_IMAGE_SOURCE_RE = re.compile(r"^\[Image(?:\]|: [^\]]*\])$")   # entrada sintetica inteira = meta
_IMAGE_MARKER_RE = re.compile(r"\[Image #\d+\]\s*")             # ruido na legenda -> remover


def _first(content: list, type_name: str) -> Optional[dict]:
    for item in content:
        if isinstance(item, dict) and item.get("type") == type_name:
            return item
    return None


# Claude Code logs slash-commands and local command I/O as synthetic "user" entries
# wrapped in these tags. They are tooling meta, not conversation — keep them out of the chat.
_COMMAND_META_PREFIXES = (
    "<command-name>", "<command-message>", "<command-args>",
    "<local-command-caveat>", "<local-command-stdout>", "<local-command-stderr>",
    "<bash-input>", "<bash-stdout>", "<bash-stderr>",
    # Invocacao de skill (/handoff, etc): o Claude Code injeta o corpo do SKILL.md como
    # entrada "user" sintetica que comeca com esta linha. E meta de tooling, nao conversa —
    # mesmo tratamento dos comandos acima (nao renderiza bubble).
    "Base directory for this skill:",
    # Notificacao de Workflow concluido: o harness injeta um <task-notification>...</task-notification>
    # como entrada "user" sintetica. Tooling meta, nao conversa — fora do chat.
    "<task-notification>",
    # Lembrete do harness ("The user named this session…", contexto de skill, etc): injetado como
    # entrada "user" sintetica. Quando vem sozinho (sem texto real), e meta — fora do chat. Quando
    # vem ANEXADO a uma msg real, _strip_meta_blocks remove so o bloco e mantem o texto do usuario.
    "<system-reminder>",
)

# Blocos de meta do harness embutidos no texto de uma msg de usuario. Removidos antes de exibir;
# se sobrar so o bloco, a msg inteira e meta e nao vira bubble.
_META_BLOCK_RE = re.compile(r"<system-reminder>.*?</system-reminder>", re.DOTALL)


def _is_command_meta(text: str) -> bool:
    return text.lstrip().startswith(_COMMAND_META_PREFIXES)


def _strip_meta_blocks(text: str) -> str:
    return _META_BLOCK_RE.sub("", text).strip()


def parse_line(line: str) -> Optional[ChatEvent]:
    line = line.strip()
    if not line:
        return None
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return None

    etype = obj.get("type")
    uid = obj.get("uuid", "")
    msg = obj.get("message")
    if not isinstance(msg, dict):
        return None
    content = msg.get("content")

    if etype == "user":
        if isinstance(content, str):
            if _is_command_meta(content):
                return None
            cleaned = _strip_meta_blocks(content)
            if not cleaned or _IMAGE_SOURCE_RE.match(cleaned):
                return None
            return ChatEvent(kind="user_msg", id=uid, text=cleaned)
        if isinstance(content, list):
            tr = _first(content, "tool_result")
            if tr is not None:
                res = tr.get("content")
                if isinstance(res, list):
                    res = " ".join(str(b.get("text", "")) for b in res if isinstance(b, dict))
                return ChatEvent(
                    kind="tool_result", id=uid,
                    tool_use_id=tr.get("tool_use_id"),
                    result=str(res) if res is not None else None,
                    is_error=bool(tr.get("is_error", False)),
                )
            # Imagens coladas no terminal: contar os blocos `image` -> o front busca cada uma lazy.
            img_count = sum(1 for it in content if isinstance(it, dict) and it.get("type") == "image")
            txt = _first(content, "text")
            t = txt.get("text", "") if txt is not None else ""
            if _is_command_meta(t):
                return None
            cleaned = _strip_meta_blocks(t)
            if _IMAGE_SOURCE_RE.match(cleaned):
                return None
            cleaned = _IMAGE_MARKER_RE.sub("", cleaned).strip()   # tira "[Image #N]" da legenda
            if not cleaned and not img_count:
                return None
            return ChatEvent(kind="user_msg", id=uid, text=cleaned,
                             image_count=img_count or None)
        return None

    if etype == "assistant" and isinstance(content, list):
        tu = _first(content, "tool_use")
        if tu is not None:
            return ChatEvent(
                kind="tool_use", id=uid,
                tool_name=tu.get("name"), tool_use_id=tu.get("id"),
                tool_input=tu.get("input") or {},
            )
        txt = _first(content, "text")
        if txt is not None:
            return ChatEvent(kind="assistant_msg", id=uid, text=txt.get("text", ""))
    return None


def path_in_transcript(jsonl: str | Path, needle: str) -> bool:
    """True se `needle` (um caminho de arquivo) aparece em ALGUMA linha do transcript. Trava de
    seguranca do endpoint de arquivo: so servimos arquivos CITADOS na conversa (consentidos) — nao
    leitura arbitraria de disco. Streaming com early-exit (nao carrega o jsonl inteiro)."""
    if not needle:
        return False
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if needle in line:
                    return True
    except OSError:
        pass
    return False


def get_transcript_image(jsonl: str | Path, uuid: str, idx: int) -> Optional[tuple[bytes, str]]:
    """Bytes + media_type da idx-ésima imagem base64 da msg de uuid no transcript, ou None.

    Fonte das imagens coladas no terminal (a image-cache do Claude não persiste). Serve sob demanda
    pra não inchar o payload do histórico/SSE com base64."""
    try:
        lines = Path(jsonl).read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return None
    for line in lines:
        try:
            obj = json.loads(line)
        except (json.JSONDecodeError, ValueError):
            continue
        if obj.get("uuid") != uuid:
            continue
        content = (obj.get("message") or {}).get("content")
        if not isinstance(content, list):
            return None
        imgs = [it for it in content if isinstance(it, dict) and it.get("type") == "image"]
        if idx < 0 or idx >= len(imgs):
            return None
        src = imgs[idx].get("source") or {}
        data = src.get("data")
        if not isinstance(data, str):
            return None
        try:
            raw = base64.b64decode(data)
        except (ValueError, base64.binascii.Error):
            return None
        media = src.get("media_type") if isinstance(src.get("media_type"), str) else "image/png"
        return raw, media
    return None


class TranscriptTailer:
    def __init__(self, path: str | Path):
        self.path = Path(path)

    def _read_from(self, pos: int) -> tuple[list[ChatEvent], int]:
        # Le do offset `pos` ate o fim -> (eventos parseados, novo offset). Sincrono de proposito:
        # chamado via asyncio.to_thread no follow() pra nao bloquear o event loop com I/O de arquivo
        # (o backfill inicial le o transcript inteiro, que cresce pra dezenas de MB em sessao longa).
        if not self.path.exists():
            return [], pos
        evs = []
        with self.path.open(encoding="utf-8", errors="replace") as fh:
            fh.seek(pos)
            for line in fh:
                ev = parse_line(line)
                if ev is not None:
                    evs.append(ev)
            return evs, fh.tell()

    async def follow(self) -> AsyncIterator[ChatEvent]:
        pos = 0
        # backfill inicial + cada append: a leitura de arquivo roda no threadpool (nao bloqueia o loop).
        evs, pos = await asyncio.to_thread(self._read_from, pos)
        for ev in evs:
            yield ev
        async for _ in awatch(self.path.parent):
            evs, pos = await asyncio.to_thread(self._read_from, pos)
            for ev in evs:
                yield ev
