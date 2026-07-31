import asyncio
import base64
import hashlib
import json
import os
import re
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import AsyncIterator, Optional
from watchfiles import awatch
from app.models import ChatEvent

# Backfill do SSE: re-envia so as ULTIMAS N linhas do transcript em cada (re)conexao, nao o arquivo
# inteiro. Antes o follow() comecava em pos=0 e re-shippava dezenas de MB a cada reconexao do mobile
# (background/foreground, watchdog). 200 e a maneta de calibracao: cobre o gap de uma reconexao normal
# (poucos segundos) com folga; sessao com <= 200 linhas mantem o backfill completo (offset 0).
_BACKFILL_LINES = 200

# Janela inicial do tail-read reverso do _tail_offset: 256KB cobre as 200 linhas do backfill na
# esmagadora maioria dos transcripts; quando nao cobre (linha gigante com base64 de imagem colada),
# ela quadruplica ate juntar as linhas ou alcancar o inicio do arquivo.
_TAIL_WINDOW = 256 * 1024

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

    # Fim de agente ENFILEIRADO. Quando o agente de background termina com o assistente no meio de um
    # turno, o harness NAO grava a <task-notification> como mensagem de user: grava uma entrada
    # `queue-operation`/enqueue, que nao tem `message` nem `uuid` — e morria no early-return logo
    # abaixo. Resultado: o painel de Atividade nunca recebia o sinal de termino e o agente ficava
    # "RODANDO AGORA" pra sempre (observado ao vivo: 2 de 6 agentes travados, os 2 que terminaram
    # enquanto o turno corria; os 4 que chegaram entre turnos vieram como user e fechavam certo).
    # Mesmo tool_result sintetico do caminho normal — `resulted` no fold e um Set, entao a entrega
    # posterior da mesma notificacao (quando vier) so repete, sem efeito.
    if etype == "queue-operation":
        queued = obj.get("content")
        if isinstance(queued, str) and queued.lstrip().startswith("<task-notification>"):
            m = _TASK_NOTIF_RE.search(queued)
            if m:
                tid = m.group(1).strip()
                # id proprio: a entrada nao tem uuid, e o front deduplica por id.
                return [ChatEvent(kind="tool_result", id=f"queued-task:{tid}",
                                  tool_use_id=f"task:{tid}", result="task-notification")]
            return []
        # Msg de usuario digitada ENQUANTO o agente trabalha: o harness enfileira (`enqueue`) e, ao
        # consumi-la DENTRO do turno em andamento, grava `remove` — nunca vira uma entrada type='user'.
        # Sem isto ela some do chat (aparece so no terminal, que conhece a fila direto). Renderiza no
        # `remove` (o consumo mid-turn): e o par EXATO das invisiveis. As que viram turno de verdade
        # saem por `dequeue` -> ja tem seu type='user' e NAO passam por aqui, entao nao duplica. id
        # pelo timestamp (a entrada nao tem uuid) pro front deduplicar por id. Mesma filtragem de meta
        # do caminho user normal (comando/skill/system-reminder/imagem sintetica nao viram bubble).
        if obj.get("operation") == "remove" and isinstance(queued, str):
            if _is_command_meta(queued):
                return []
            cleaned = _strip_meta_blocks(queued)
            if not cleaned or _IMAGE_SOURCE_RE.match(cleaned):
                return []
            cleaned = _IMAGE_MARKER_RE.sub("", cleaned).strip()   # tira "[Image #N]" da legenda
            if not cleaned:
                return []
            # id = timestamp + hash do conteudo. So o timestamp NAO basta: duas msgs consumidas no
            # MESMO instante (medido: "no caso..." e "so pra..." removidas as 17:16:37) colidiriam e o
            # front, que deduplica por id, esconderia uma. Hash estavel (nao o hash() randomizado do
            # processo) pra o mesmo remove reparseado manter o id e nao duplicar na reconexao do SSE.
            digest = hashlib.md5(queued.encode("utf-8", "replace")).hexdigest()[:8]
            return [ChatEvent(kind="user_msg",
                              id=f"queued:{obj.get('timestamp', '')}:{digest}", text=cleaned)]
        return []

    msg = obj.get("message")
    if not isinstance(msg, dict):
        return []
    content = msg.get("content")

    if etype == "user":
        # Entrada sintetica que o proprio Claude Code marca com isMeta: expansao de slash-command/
        # skill (o corpo do comando vira "mensagem do usuario"), prompt injetado de loop/cron,
        # "Continue from where you left off", avisos de hook. No terminal isso nao aparece; aqui
        # viraria bubble e poluiria o chat. Fora do chat. (Os <command-*> tags e a task-notification
        # NAO vem com isMeta -> seguem tratados abaixo pelo caminho de sempre.)
        if obj.get("isMeta") is True:
            return []
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
        cache_read, ttl = _cache_info(msg)
        ts = _ts(obj)
        out = []
        for it in content:
            if not isinstance(it, dict):
                continue
            if it.get("type") == "tool_use":
                out.append(ChatEvent(
                    kind="tool_use", id=_sub_id(uid, len(out)),
                    tool_name=it.get("name"), tool_use_id=it.get("id"),
                    tool_input=it.get("input") or {}, ts=ts,
                ))
            elif it.get("type") == "text":
                out.append(ChatEvent(kind="assistant_msg", id=_sub_id(uid, len(out)),
                                     text=it.get("text", ""), ts=ts,
                                     cache_read=cache_read, cache_ttl_s=ttl))
        return out
    return []


def _ts(obj: dict) -> Optional[float]:
    """Epoch (segundos) do `timestamp` ISO da entrada. O campo `ts` do ChatEvent existia desde
    sempre e NUNCA era preenchido — por isso a hora nao aparecia em bubble nenhuma."""
    raw = obj.get("timestamp")
    if not isinstance(raw, str) or not raw:
        return None
    try:
        dt = datetime.fromisoformat(raw.replace("Z", "+00:00"))
    except ValueError:
        return None
    # Sem fuso no texto, .timestamp() assume o fuso LOCAL do processo -> epoch deslocado (3h aqui),
    # calado. O transcript escreve UTC; assumimos UTC em vez de herdar o fuso da maquina.
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.timestamp()


def _cache_info(msg: dict) -> tuple[Optional[int], Optional[int]]:
    """(tokens lidos do cache, TTL em segundos) do usage do turno.

    O TTL vem MEDIDO, nao suposto: `usage.cache_creation` separa `ephemeral_1h_input_tokens` de
    `ephemeral_5m_input_tokens`, entao da pra dizer qual janela a sessao esta usando. Sem esse
    detalhe (formato antigo), devolve None em vez de chutar 5min — melhor nao mostrar prazo do que
    mostrar um prazo errado."""
    usage = msg.get("usage")
    if not isinstance(usage, dict):
        return None, None
    read = usage.get("cache_read_input_tokens")
    read = int(read) if isinstance(read, (int, float)) else None
    creation = usage.get("cache_creation")
    ttl: Optional[int] = None
    if isinstance(creation, dict):
        # int() sem guarda de tipo levantava aqui com qualquer valor nao-numerico, e a excecao subia
        # ate derrubar a SSE. Como o backfill relê as ultimas linhas a cada reconexao, UMA linha
        # estranha viraria loop de queda pra aquela sessao.
        if _tok(creation.get("ephemeral_1h_input_tokens")) > 0:
            ttl = 3600
        elif _tok(creation.get("ephemeral_5m_input_tokens")) > 0:
            ttl = 300
    return read, ttl


def _tok(v: object) -> int:
    return int(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else 0


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


def last_assistant_text(jsonl: str | Path) -> Optional[str]:
    """Texto do ULTIMO evento de assistant do transcript (modo done_claimed do loop procura
    'LOOP_DONE' aqui). Streaming linha a linha (padrao path_in_transcript); None se ausente."""
    last: Optional[str] = None
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                for ev in parse_line(line):
                    if ev.kind == "assistant_msg" and ev.text:
                        last = ev.text
    except OSError:
        return None
    return last


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
    def __init__(self, path: str | Path, parse_line=parse_line):
        self.path = Path(path)
        # Parser injetavel: default e o parse_line do Claude (snake_case), mas o CodexAdapter
        # reaproveita a mesma mecanica de tail (backfill + watch de append) passando
        # parse_rollout_line (shape do rollout do Codex e diferente).
        self._parse_line = parse_line

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
            start = pos
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
                parsed = self._parse_line(line.decode("utf-8", "replace"))
                for ev in parsed:
                    # Offset do INICIO da linha, nao do fim: uma linha pode render VARIOS eventos e
                    # eles compartilham o id. Se o cliente recebeu so o 1o e reconectou, retomar
                    # pelo fim PULARIA os irmaos. Pelo inicio a linha e relida inteira e o front
                    # descarta o que ja tem (dedup por ev.id) -- sobreposicao barata, perda zero.
                    ev.offset = start
                evs.extend(parsed)
            # Parser com memoria (o do Pi segura o user_msg por uma linha, ver adapters/pi/
            # transcript.Stream): o que ficou retido sai no fim do LOTE, nao na proxima leitura —
            # senao a mensagem do usuario so apareceria quando o Pi gravasse a linha seguinte, que
            # num turno longo demora minutos.
            owner = getattr(self._parse_line, "__self__", None)
            if hasattr(owner, "flush_events"):
                evs.extend(self._flush_com_espera(fh, owner, start))
            return evs, fh.tell()

    # Espera curta antes de soltar o que o parser reteve. As duas linhas do par (mensagem do
    # usuario + o marcador do hook que diz o que nela e do hook) nascem no MESMO milissegundo, mas
    # nada garante que o watcher acorde depois das duas: se o lote fechar entre elas, a bolha sai
    # sem o corte e o marcador chega orfao no lote seguinte — volta calada ao bug antigo. Uma
    # releitura curta fecha essa fresta sem atrasar nada perceptivel, e so roda quando ha algo
    # retido (Claude/Codex nunca retem). ponytail: 200ms e o knob — medido 1ms entre as duas
    # escritas do Pi, entao a folga e de duas ordens de grandeza.
    _ESPERA_PAR_S = 0.2

    def _flush_com_espera(self, fh, owner, start: int) -> list[ChatEvent]:
        """Releitura curta e depois solta o retido. Devolve (linhas novas + o que sobrou retido)."""
        if not owner.tem_retido():
            return owner.flush_events()          # nada preso: sem espera nenhuma
        time.sleep(self._ESPERA_PAR_S)
        novos: list[ChatEvent] = []
        while True:
            ini = fh.tell()
            line = fh.readline()
            if not line or not line.endswith(b"\n"):
                fh.seek(ini)                     # EOF ou linha pela metade: fica pro proximo ciclo
                break
            for ev in self._parse_line(line.decode("utf-8", "replace")):
                ev.offset = ini
                novos.append(ev)
            start = ini
        held = owner.flush_events()
        for ev in held:
            ev.offset = start        # inicio da ULTIMA linha lida (a que segurou o release)
        return novos + held

    def _size(self) -> int:
        try:
            return self.path.stat().st_size
        except OSError:
            return 0

    def _tail_offset(self, max_lines: int) -> int:
        # Offset do inicio da (max_lines)-esima linha a partir do fim -> o follow() faz backfill so do
        # tail. Le do FIM pra tras (mesmo desenho do _tail_offset do pqueue): varrer pra frente
        # custava o arquivo inteiro -- 136MB lidos pra pular pros ultimos ~500KB, em todo connect de
        # SSE sem Last-Event-ID. <= max_lines linhas, arquivo vazio ou ausente -> 0 (backfill do
        # inicio = comportamento antigo).
        #
        # Conta so `\n`: a linha completa k comeca depois do k-esimo `\n`, entao o inicio da
        # max_lines-esima a partir do fim fica logo apos o (max_lines+1)-esimo `\n` contado de tras
        # pra frente. Cauda sem `\n` (append em voo) nao entra na conta nem desloca nada, igual antes.
        try:
            with self.path.open("rb") as fh:
                size = fh.seek(0, os.SEEK_END)
                window = _TAIL_WINDOW
                while True:
                    start = max(0, size - window)
                    fh.seek(start)
                    buf = fh.read(size - start)
                    if buf.count(b"\n") > max_lines:
                        idx = len(buf)
                        for _ in range(max_lines + 1):
                            idx = buf.rindex(b"\n", 0, idx)
                        return start + idx + 1
                    if start == 0:
                        return 0     # arquivo inteiro na janela e ainda nao deu max_lines linhas
                    window *= 4      # janela curta (ou uma linha gigante, base64 de imagem): cresce
        except OSError:
            return 0

    async def follow(self, start_offset: int | None = None) -> AsyncIterator[ChatEvent]:
        """Backfill + watch de append. `start_offset` (do Last-Event-ID) retoma EXATAMENTE dali.

        Sem ele, backfill so do TAIL (ultimas _BACKFILL_LINES linhas). Essa janela cobre ~2 min de
        trabalho pesado (medido: mediana 44 linhas/min, pico 133) -- uma queda de celular mais longa
        que isso perdia o miolo do buraco. Com o offset o resume e exato e barato (nao reenvia 200
        linhas a cada reconexao). Offset invalido (arquivo trocado/truncado) cai no tail de sempre.
        """
        if start_offset is not None:
            size = await asyncio.to_thread(self._size)
            # Alem do EOF = transcript trocado ou truncado sob o cliente -> o offset nao significa
            # mais nada. Volta pro tail em vez de retomar no lugar errado (ou reler o arquivo todo).
            pos = start_offset if 0 <= start_offset <= size else None
        else:
            pos = None
        if pos is None:
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
