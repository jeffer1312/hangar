import asyncio
import base64
import json
import os
import re
from collections import deque
from pathlib import Path
from typing import AsyncIterator, Optional
from watchfiles import awatch
from app.models import ChatEvent

# Backfill do SSE: re-envia so as ULTIMAS N linhas do transcript em cada (re)conexao, nao o arquivo
# inteiro. Antes o follow() comecava em pos=0 e re-shippava dezenas de MB a cada reconexao do mobile
# (background/foreground, watchdog). 200 e a maneta de calibracao: cobre o gap de uma reconexao normal
# (poucos segundos) com folga; sessao com <= 200 linhas mantem o backfill completo (offset 0).
_BACKFILL_LINES = 200

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

# task-id de uma <task-notification> (fim de agente/workflow em background). A notificacao fica
# fora do chat (e ruido), mas o painel de Atividade precisa do sinal de termino: viram um
# tool_result SINTETICO com tool_use_id="task:<id>" (o front nunca renderiza tool_result orfao;
# so o fold de atividade consome).
_TASK_NOTIF_RE = re.compile(r"<task-id>([^<]+)</task-id>")


def _is_command_meta(text: str) -> bool:
    return text.lstrip().startswith(_COMMAND_META_PREFIXES)


def _strip_meta_blocks(text: str) -> str:
    return _META_BLOCK_RE.sub("", text).strip()


def parse_line(line: str) -> list[ChatEvent]:
    line = line.strip()
    if not line:
        return []
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return []
    return parse_obj(obj)


def _sub_id(uid: str, k: int) -> str:
    # Eventos extras da MESMA linha ganham sufixo deterministico (":1", ":2"...): o front deduplica
    # e keia bubble por id -> ids repetidos colapsariam blocos distintos numa bubble so. O 1o fica
    # com o uuid puro (o fetch de imagem usa o id cru como uuid da entrada no jsonl).
    return uid if k == 0 else f"{uid}:{k}"


def parse_obj(obj: dict) -> list[ChatEvent]:
    """Eventos de chat de UMA entrada (ja parseada) do transcript. Lista pq uma entrada pode
    carregar VARIOS blocos (tool calls paralelas = varios tool_result numa msg user so; assistant
    com text + tool_use juntos) — devolver so o 1o engolia os demais silenciosamente."""
    etype = obj.get("type")
    uid = obj.get("uuid", "")
    msg = obj.get("message")
    if not isinstance(msg, dict):
        return []
    content = msg.get("content")

    if etype == "user":
        if isinstance(content, str):
            if content.lstrip().startswith("<task-notification>"):
                m = _TASK_NOTIF_RE.search(content)
                if m:
                    return [ChatEvent(kind="tool_result", id=uid,
                                      tool_use_id=f"task:{m.group(1).strip()}",
                                      result="task-notification")]
                return []
            if _is_command_meta(content):
                return []
            cleaned = _strip_meta_blocks(content)
            if not cleaned or _IMAGE_SOURCE_RE.match(cleaned):
                return []
            return [ChatEvent(kind="user_msg", id=uid, text=cleaned)]
        if isinstance(content, list):
            trs = [it for it in content if isinstance(it, dict) and it.get("type") == "tool_result"]
            if trs:
                out = []
                for k, tr in enumerate(trs):
                    res = tr.get("content")
                    if isinstance(res, list):
                        res = " ".join(str(b.get("text", "")) for b in res if isinstance(b, dict))
                    out.append(ChatEvent(
                        kind="tool_result", id=_sub_id(uid, k),
                        tool_use_id=tr.get("tool_use_id"),
                        result=str(res) if res is not None else None,
                        is_error=bool(tr.get("is_error", False)),
                    ))
                return out
            # Imagens coladas no terminal: contar os blocos `image` -> o front busca cada uma lazy.
            img_count = sum(1 for it in content if isinstance(it, dict) and it.get("type") == "image")
            txt = _first(content, "text")
            t = txt.get("text", "") if txt is not None else ""
            if _is_command_meta(t):
                return []
            cleaned = _strip_meta_blocks(t)
            if _IMAGE_SOURCE_RE.match(cleaned):
                return []
            cleaned = _IMAGE_MARKER_RE.sub("", cleaned).strip()   # tira "[Image #N]" da legenda
            if not cleaned and not img_count:
                return []
            return [ChatEvent(kind="user_msg", id=uid, text=cleaned,
                              image_count=img_count or None)]
        return []

    if etype == "assistant" and isinstance(content, list):
        # Um evento POR BLOCO, na ordem do content (thinking etc. ignorados). Antes o 1o tool_use
        # vencia e um bloco text na mesma entrada sumia do chat.
        out = []
        for it in content:
            if not isinstance(it, dict):
                continue
            if it.get("type") == "tool_use":
                out.append(ChatEvent(
                    kind="tool_use", id=_sub_id(uid, len(out)),
                    tool_name=it.get("name"), tool_use_id=it.get("id"),
                    tool_input=it.get("input") or {},
                ))
            elif it.get("type") == "text":
                out.append(ChatEvent(kind="assistant_msg", id=_sub_id(uid, len(out)),
                                     text=it.get("text", "")))
        return out
    return []


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
        fh = Path(jsonl).open(encoding="utf-8", errors="replace")
    except OSError:
        return None
    with fh:  # streaming linha-a-linha: nao carrega o transcript inteiro (dezenas de MB) em RAM
        for line in fh:
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
        # Binario: tell()/seek() em modo texto sao cookies opacos (nao offsets em bytes) -> nao
        # daria pra comparar com st_size no guard de truncamento; o decode fica por linha lida.
        if not self.path.exists():
            return [], pos
        evs: list[ChatEvent] = []
        with self.path.open("rb") as fh:
            if os.fstat(fh.fileno()).st_size < pos:
                # arquivo ENCOLHEU (truncado/reescrito): o offset antigo cairia alem do EOF e a
                # leitura retomaria no meio de linha nova = lixo/eventos perdidos. Recomeca do
                # zero (o arquivo pos-truncamento e pequeno; o front deduplica por id).
                pos = 0
            fh.seek(pos)
            while True:
                start = fh.tell()
                line = fh.readline()
                if not line:
                    break
                if not line.endswith(b"\n"):
                    # awatch disparou no meio de um append -> linha incompleta. Rebobina pro inicio
                    # dela e nao avanca pos: a versao COMPLETA e relida no proximo evento do watcher.
                    fh.seek(start)
                    break
                evs.extend(parse_line(line.decode("utf-8", "replace")))
            return evs, fh.tell()

    def _tail_offset(self, max_lines: int) -> int:
        # Offset do inicio da (max_lines)-esima linha a partir do fim -> o follow() faz backfill so do
        # tail. Conta LINHAS completas (terminadas em \n) sem parsear JSON, em binario (sem decodar o
        # arquivo inteiro) e com deque(maxlen) (nao acumula o offset de TODAS as linhas).
        # <= max_lines linhas, ou arquivo ausente -> 0 (backfill do inicio = comportamento antigo).
        # ponytail: varre o arquivo pra frente sem parse; reverse-seek so se o disco virar gargalo.
        if not self.path.exists():
            return 0
        starts: deque[int] = deque(maxlen=max_lines)
        with self.path.open("rb") as fh:
            while True:
                start = fh.tell()
                line = fh.readline()
                if not line:
                    break
                if not line.endswith(b"\n"):
                    break  # ultima linha incompleta (append em voo): ignora, nao registra o start
                starts.append(start)
        # deque cheio = havia >= max_lines linhas; starts[0] e o inicio da max_lines-esima a partir
        # do fim (com EXATAMENTE max_lines linhas, starts[0] == 0 = backfill completo, como antes).
        return starts[0] if len(starts) == max_lines else 0

    async def follow(self) -> AsyncIterator[ChatEvent]:
        # Backfill so do TAIL (ultimas _BACKFILL_LINES linhas), nao o arquivo inteiro. _tail_offset
        # devolve 0 quando ha poucas linhas -> sessao curta mantem o backfill completo. A leitura roda
        # no threadpool (nao bloqueia o loop); custo <= o _read_from(0) de antes (varredura sem parse).
        pos = await asyncio.to_thread(self._tail_offset, _BACKFILL_LINES)
        # backfill inicial + cada append: a leitura de arquivo roda no threadpool (nao bloqueia o loop).
        evs, pos = await asyncio.to_thread(self._read_from, pos)
        for ev in evs:
            yield ev
        # yield_on_timeout: alem dos eventos do FS, acorda a cada rust_timeout mesmo sem mudanca
        # (changes vazio) e rele -> fecha a janela morta entre o backfill acima e o watcher armar
        # (evento gravado nesse gap so apareceria no proximo write) e cobre inotify perdido.
        async for changes in awatch(self.path.parent, yield_on_timeout=True, rust_timeout=5000):
            # O watch e do DIRETORIO (o proprio arquivo pode nem existir ainda), mas escrita de
            # jsonl IRMAO (ex: subagente gravando o proprio transcript ao lado) acordava todos os
            # tailers -> so rele quando o toque e no NOSSO arquivo (ou no timeout do heartbeat).
            if changes and not any(Path(p).name == self.path.name for _, p in changes):
                continue
            evs, pos = await asyncio.to_thread(self._read_from, pos)
            for ev in evs:
                yield ev
