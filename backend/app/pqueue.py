import asyncio
import difflib
import functools
import inspect
import json
import logging
import os
import re
import threading
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import AsyncIterator, Callable

from watchfiles import awatch

from app import atomico
from app.config import settings
from app.models import ChatEvent, dumps_safe, scrub_surrogates
from app.transcript import RewriteFilter, parse_obj, silent_attachment_timestamp

_log = logging.getLogger("hangar.pqueue")

# Limite de entradas mantidas no sidecar (poda no append pra nao crescer sem fim).
_MAX_ENTRIES = 1000


def _queue_dir() -> Path:
    # Sidecar FORA do transcript do Claude Code (nunca toca no arquivo dele). Fica ao lado de
    # projects/, no diretorio de config (~/.claude-work por padrao).
    d = Path(settings.projects_dir).parent / ".hangar-queue"
    d.mkdir(parents=True, exist_ok=True)
    return d


_append_lock = threading.Lock()  # serializa o read-modify-write do append (handlers sync no threadpool)


def _sanitize(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "-", name)


_PREFIXO_MIN = 8   # piso de prefixo: "ok"/"sim"/"1" nao confirmam frase alheia que comeca igual


_RUN_CONTRABARRA = re.compile(r"\\\\+")


def _encolhe_contrabarra(s: str) -> str:
    """O texto como o transcript o guarda quando o psmux comeu contrabarra no caminho.

    No Windows o `send-keys -l` leva o texto no argv, e o psmux desescapa: corrida de N
    contrabarras chega com ceil(N/2) (medido byte a byte em 07/09/2026 — 2 viram 1, 3 viram 2).
    A prova de entrega compara STRING, entao o eco corrompido nunca casava com a entrada e a msg
    virava bolha de 'nao chegou' sobre uma msg que CHEGOU — e o RESGATE, que existe justamente
    pra desfazer isso, tambem nao disparava.

    Aplicar a mesma regra do lado da ENTRADA da fila devolve a forma que o transcript tem. Isto
    nao e conserto da origem (esse mora no terminal_input, que passou a colar pelo clipboard):
    e tolerancia pro que JA aconteceu e esta gravado na fila.

    Sem contrabarra no texto, devolve o proprio objeto — nenhuma mensagem normal paga nada, e no
    Linux (onde o argv nao come nada) a forma extra so nunca casa com linha nenhuma.
    """
    if "\\" not in s:
        return s
    return _RUN_CONTRABARRA.sub(lambda m: "\\" * -(-len(m.group(0)) // 2), s)


def _linhas_da_entrada(r: dict) -> set[str]:
    """Formas do texto de UMA entrada da fila: cru, sem o marcador de anexo, e cada uma por linha.

    As duas formas sao as mesmas em dois pontos do reconcile (resgate e caminho normal) — um
    helper unico evita que um dos lados normalize diferente do outro e casamentos divergirem.
    """
    cru = str(r.get("text") or "").strip()
    podado = _strip_attach(cru).strip()
    ls = {cru, podado,
          *(ln.strip() for ln in cru.split("\n")),
          *(ln.strip() for ln in podado.split("\n"))}
    # Formas ENCOLHIDAS junto: no Windows o eco pode ter chegado com contrabarra a menos (ver
    # _encolhe_contrabarra), e sem elas nem o casamento normal nem o resgate casavam.
    ls |= {_encolhe_contrabarra(x) for x in ls}
    ls.discard("")
    return ls


def _dono_do_prefixo(rows: list[dict], disponiveis: set[str]) -> dict[str, str]:
    """Linha do transcript -> o MAIOR prefixo que alguma entrada pendente reivindica nela.

    `reservadas` resolve exato-contra-prefixo. Isto resolve prefixo-contra-prefixo: com o eco
    chegando com SUFIXO, NENHUMA das entradas casa exato (reservadas fica vazia) e a entrada mais
    CURTA levava a linha da mais especifica — invertendo as duas marcas de novo (X perdida vira
    entregue, Y que chegou ganha 'nao chegou'). A regra das duas e uma so: a linha pertence a quem
    a reivindica de forma MAIS ESPECIFICA — exato ganha de prefixo; prefixo mais longo ganha de
    prefixo mais curto.
    """
    dono: dict[str, str] = {}
    for r in rows:
        if r.get("delivered") is not True or r.get("confirmed"):
            continue
        for ln in _linhas_da_entrada(r):
            if len(ln) < _PREFIXO_MIN:
                continue
            for d in disponiveis:
                if d.startswith(ln) and len(ln) > len(dono.get(d, "")):
                    dono[d] = ln
    return dono


# Folga pra entrada desistida ANTES de `desistiu_ts` existir: sem o instante gravado, "reenvio" e
# o mesmo texto mandado bem depois. Duas respostas iguais de picker saem em segundos; um reenvio
# humano depois de ler "nao chegou" leva mais que isso.
_REENVIO_FOLGA_LEGADO = 60.0


def _tem_reenvio(r: dict, rows: list[dict]) -> bool:
    """Ha outra entrada com o MESMO texto mandada depois de `r` desistir?"""
    texto = str(r.get("text") or "").strip()
    if not texto:
        return False
    limiar = r.get("desistiu_ts")
    if not isinstance(limiar, (int, float)):
        limiar = float(r.get("ts") or 0.0) + _REENVIO_FOLGA_LEGADO
    return any(o is not r and str(o.get("text") or "").strip() == texto
               and float(o.get("ts") or 0.0) > limiar
               for o in rows)


def _casam(linhas: set[str], disponiveis: set[str],
           reservadas: frozenset[str] = frozenset(),
           dono: dict[str, str] | None = None) -> set[str]:
    """Linhas do transcript que `linhas` (de UMA entrada da fila) cobrem: iguais + prefixos.

    O eco pode chegar com SUFIXO — medido em 18/08/2026: a fila digitou "Vamos fazer ate as 23
    com o Deepseek..." e o transcript gravou a MESMA linha com "… eu tinha mandado isso" no fim.
    O casamento por linha exata nunca casava, a entrega desistia e a bolha ficava marcada 'nao
    chegou' sobre uma msg que CHEGOU. Piso de comprimento pro prefixo: resposta curta nao
    confirma frase alheia.

    Prioridade (parecer G2 rev2, bloqueador 1): a linha do transcript pertence a quem a
    reivindica de forma MAIS ESPECIFICA. `reservadas` = linha que casa EXATO com outra entrada
    pendente pertence a ela (prefixo nenhum a leva). `dono` = linha com sufixo pertence ao MAIOR
    prefixo que a reivindica (sem isto, "Vamos fazer" comeria a linha de "Vamos fazer ate as 23
    com o Deepseek" e as marcas invertiam). E o prefixo consome UMA linha por linha da fila (a
    mais curta = a menos abrangente), nao todas.
    """
    consumidas: set[str] = set()
    for ln in linhas:
        if ln in disponiveis:
            consumidas.add(ln)
        elif len(ln) >= _PREFIXO_MIN:
            cands = sorted((d for d in disponiveis
                            if d.startswith(ln) and d not in reservadas
                            and (dono is None or dono.get(d) == ln)),
                           key=len)
            if cands:
                consumidas.add(cands[0])
    return consumidas


# Folga que a marca `pre_transcript` compra sobre o corte por idade. 15 min cobre com sobra o
# intervalo real entre enfileirar o kick-off e a sessão nova gravar a 1a linha do transcript
# (segundos), e é o que impede a isenção de virar ETERNA: a chave fica no disco pra sempre (o
# prune_before é justamente quem não a apaga), então uma isenção sem prazo faria um kick-off nunca
# entregue — sessão morta antes de a TUI aceitar texto — ressuscitar numa VIDA POSTERIOR da sessão
# de mesmo nome, que é a dívida da sessão anterior que o corte existe pra matar. Passada a folga
# ela é lixo como qualquer outra entrada velha: some no prune_before e o cheap-check do drain esfria.
_JANELA_BASTAO = 15 * 60.0


def _da_sessao_atual(entry: dict, min_ts: float, ts: float | None = None) -> bool:
    """A entrada pertence à sessão de AGORA (ou está dentro da folga do bastão)?

    O corte normal é `ts >= min_ts`: entrada carimbada antes do início do transcript é de uma vida
    anterior (pré-`/clear`, ou o `pi -c` que reusa transcript velho) e não pode ser entregue nem
    reaparecer como bolha.

    `ts` explícito: o `merged_history` já resolve o relógio da entrada com carry-forward (entrada
    sem `ts` herda o da linha anterior) e passa esse valor pra cá, senão a MESMA entrada teria duas
    idades dentro da mesma função — ordenada pelo relógio herdado e cortada por 0.0.

    `pre_transcript` é a exceção, e ela existe pra UM caso: a passagem de bastão enfileira o
    kick-off ANTES de a sessão nova existir — logo, antes da primeira linha do `.jsonl` e antes do
    nascimento do tmux. Sem esta marca as duas redes de segurança da fila comem a entrada em
    silêncio (o `prune_before` do drain a APAGA como vida anterior; o `reconcile_delivered` a
    carimba `confirmed` como "sessão anterior"), e o resultado é o pior possível: dossiê gravado,
    sessão nascida, sucessor sem receber nada e nenhum erro em lugar nenhum.

    A marca isenta só do corte por IDADE, e só por `_JANELA_BASTAO` — a entrada segue passando pelo
    reconcile normal, então kick-off engolido pela TUI ainda é reentregue e ainda desiste no teto
    de tentativas.
    """
    folga = _JANELA_BASTAO if entry.get("pre_transcript") else 0.0
    quando = float(entry.get("ts") or 0.0) if ts is None else ts
    return quando >= min_ts - folga


# Nota local gravada até isto antes de o stream abrir ainda sai por ele: cobre a que nasce entre o
# /history do front e a conexão do SSE.
_FOLGA_SAIDA_LOCAL_S = 30.0


def _saida_local(entry: dict) -> bool:
    # Entrada que NAO e prompt: texto que o agente respondeu fora do transcript (comando local
    # da sessao sem terminal). Nasce entregue e confirmada — drain e reconcile nunca a tocam — e
    # vira bolha do assistente com id "local-", que o front trata como mensagem comum.
    return entry.get("papel") == "assistant"


def _entry_event(entry: dict) -> ChatEvent:
    if _saida_local(entry):
        return ChatEvent(kind="assistant_msg", id="local-" + str(entry.get("id")), text=entry.get("text"))
    # user_msg sintetico com id prefixado ("queued-") pro front distinguir de evento real do
    # transcript. ts fica None de proposito: o ts so serve pra ORDENAR no historico, nao pra
    # exibir (senao bubble enfileirada mostraria hora e as do transcript nao -> inconsistente).
    # `desistiu` vai junto: e a UNICA forma de o front distinguir "esperando a vez" de "perdida".
    # Sem ele a bolha desistida acendia solida igual a uma aceita (ver models.ChatEvent.desistiu).
    # `queued_ts` é o relógio da entrada pra ORDENAR: a reconexão do SSE reemite a fila inteira, e
    # sem ele uma desistida de horas atrás era anexada no fim como se fosse nova.
    ts = entry.get("ts")
    return ChatEvent(kind="user_msg", id="queued-" + str(entry.get("id")), text=entry.get("text"),
                     queued_delivered=entry.get("delivered") if isinstance(entry.get("delivered"), bool) else None,
                     desistiu=True if entry.get("desistiu") else None,
                     queued_ts=float(ts) if isinstance(ts, (int, float)) else None)


def _ts_of_obj(obj: dict) -> float:
    # Epoch (s) do campo `timestamp` (ISO 8601 com Z) de uma entrada do transcript; 0.0 se ausente.
    t = obj.get("timestamp")
    if isinstance(t, str):
        try:
            return datetime.fromisoformat(t.replace("Z", "+00:00")).timestamp()
        except ValueError:
            return 0.0
    # Pi: sem `timestamp` ISO no topo, o numero (epoch ms) mora dentro de `message`.
    msg = obj.get("message")
    if isinstance(msg, dict):
        raw = msg.get("timestamp")
        if isinstance(raw, (int, float)):
            return raw / 1000.0
    # Kimi (wire.jsonl): epoch ms no envelope `time` da linha (`created_at` na linha metadata,
    # a 1a do arquivo — sem ela o start_ts escorregava pro 1o turno).
    for k in ("time", "created_at"):
        raw = obj.get(k)
        if isinstance(raw, (int, float)):
            return raw / 1000.0
    return 0.0


def _ts_of_line(line: str) -> float:
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return 0.0
    return _ts_of_obj(obj)


def _transcript_start_ts(jsonl: str) -> float | None:
    # ts (epoch) da 1a linha COM timestamp do transcript = inicio da sessao atual. Toda entrada da
    # fila mais antiga que isto pertence a uma sessao anterior (ex: pre-/clear, que cria transcript
    # novo com novo session-id) e nao deve reaparecer como bubble. Le so ate achar o 1o ts (early
    # return) pra nao varrer transcript gigante. 0.0 se nao houver ts -> sem poda (fallback seguro).
    #
    # None = NAO DEU PRA LER, e e diferente de 0.0 pelo mesmo motivo que separa `None` de set()
    # em committed_user_lines: 0.0 desliga a poda, e sem poda uma entrada de sessao ANTERIOR deixa
    # de ser dispensada por idade e cai no caminho normal do reconcile — se o texto dela nao casar
    # com o transcript de AGORA (e nao vai, e de outra sessao), ela e re-enfileirada e REDIGITADA.
    # Ou seja: o mesmo defeito que a funcao irma acabou de perder, entrando pela porta do lado.
    # Quem so PODA (sse, drain, merged_history) aceita 0.0 no lugar de None e escreve isso no
    # proprio call site — ali "nao sei" pode virar "nao corta" sem estrago.
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                ts = _ts_of_line(line)
                if ts > 0:
                    return ts
    except OSError as e:
        _log.warning("nao deu pra ler o inicio do transcript %s: %s", jsonl, e)
        return None
    return 0.0


# Marcador de anexo no texto do app ("legenda — 📎 imagem: <path>"): o transcript grava SO a
# legenda -> tirar antes de casar com o transcript (senao msg com anexo nunca confirmaria).
_ATTACH_RE = re.compile(r"(?:\s*—\s*)?📎\s*(?:imagem|arquivo):.*$", re.S)


def _strip_attach(text: str) -> str:
    return _ATTACH_RE.sub("", text)


# Prefixo que o Claude Code PREPENDA ao prompt quando o texto referencia imagem anexada
# ("[Image #1]<texto>"; multiplas imagens empilham). A fila guarda o texto SEM ele.
_IMG_PREFIX = re.compile(r"^(?:\[Image #\d+\])+\s*")
# Mensagem SO de imagem: o Claude Code grava o caminho como "[Image: source: <path>]" em vez do
# "📎 imagem: <path>" que o app digitou (2.1.270). Sem traduzir de volta, a entrega nunca casava e
# o print era redigitado.
_IMG_SOURCE = re.compile(r"\[Image: source: ([^\]]+)\]")
# `/comando args` digitado vira `<command-name>/comando</command-name>` + `<command-args>` no transcript.
_COMMAND_NAME = re.compile(r"<command-name>([^<]*)</command-name>")
_COMMAND_ARGS = re.compile(r"<command-args>(.*?)</command-args>", re.S)
# Recados que chegam durante um turno viram UM registro com um bloco por recado: cada corpo é uma entrega.
_PEER_BODY = re.compile(r"<cross-session-message\b[^>]*>\n?(.*?)\n?</cross-session-message>", re.S)
_PEER_STARTS = ("<cross-session-message", "Another Claude session sent a message:")


def _chaves_de_commit(text: str) -> set[str]:
    """Todas as formas sob as quais `text` (um user_msg JA no transcript) pode ser reconhecido: cru,
    sem o "[Image #N]", sem o marcador de anexo, e cada uma dessas por LINHA.

    E a MESMA normalizacao do committed_user_lines — aqui ela serve o dedup do merged_history, que
    comparava so o texto CRU. Com anexo os dois lados nunca sao iguais: a fila guarda
    "legenda — 📎 imagem: /a.png /b.png", e o Claude Code reescreve o prompt (quebra linha depois do
    marcador e CONSOME o path da imagem que virou anexo de verdade). Resultado medido em 03/08/2026:
    a mesma mensagem aparecia DUAS vezes no historico — a bolha da fila (com as miniaturas) e a real
    — ate o reconcile do idle marcar `confirmed`. Ou seja: duplicada durante o turno inteiro, que e
    justamente quando a pessoa esta olhando."""
    out: set[str] = set()
    t = text.strip()
    base = _IMG_PREFIX.sub("", t)
    fonte = _IMG_SOURCE.sub(lambda m: f"📎 imagem: {m.group(1)}", t)
    # Só a mensagem que É o comando conta (citar a tag não conta), e ele entra inteiro, sem quebrar por linha.
    nome = _COMMAND_NAME.search(t) if t.startswith(("<command-name>", "<command-message>")) else None
    if nome:
        args = _COMMAND_ARGS.search(t)
        comando = f"{nome.group(1).strip()} {args.group(1).strip() if args else ''}".strip()
        if comando:
            out.add(comando)
    if t.startswith(_PEER_STARTS):
        out.update(b.strip() for b in _PEER_BODY.findall(t) if b.strip())
    for variant in (t, base, _strip_attach(t), _strip_attach(base), fonte):
        variant = variant.strip()
        if not variant:
            continue
        out.add(variant)
        for ln in variant.split("\n"):
            ln = ln.strip()
            if ln:
                out.add(ln)
    return out


def fila_interna_pendente(jsonl: str, provider: str = "claude", *,
                          min_offset: int = 0, prefix_digest: str | None = None,
                          imported_item_ids: tuple[str, ...] = ()) -> set[str]:
    """Textos que o Claude Code ENFILEIROU (`queue-operation`/enqueue) e ainda nao consumiu.

    Entre o enqueue e o dequeue/remove a mensagem existe so na fila interna da TUI: o
    `committed_user_lines` a conta como aterrissada (pra nao redigitar), e o parser nao a
    renderiza (pra nao duplicar com o dequeue) — ou seja, ela sumia da tela ao recarregar. O
    reconcile usa isto pra deixar a entrada entregue-e-nao-confirmada (bolha da fila visivel)
    ate o consumo aparecer no transcript. Falha de leitura vira set vazio: aqui "nao sei" so
    deixa de SEGURAR a confirmacao, nunca redigita nem esconde nada por si.
    """
    if provider != "claude":
        return set()
    lido = _ler_indice(jsonl, provider, min_offset=min_offset, prefix_digest=prefix_digest,
                       imported_item_ids=imported_item_ids)
    if lido is None:
        return set()
    out: set[str] = set()
    for t in lido[1]:
        base = _IMG_PREFIX.sub("", t)
        fonte = _IMG_SOURCE.sub(lambda m: f"📎 imagem: {m.group(1)}", t)
        for variant in (t, base, _strip_attach(t), _strip_attach(base), fonte):
            variant = variant.strip()
            if variant:
                out.add(variant)
                out.update(ln.strip() for ln in variant.split("\n"))
    return out


class _CommittedIndex:
    """Leitura incremental do transcript pra confirmar entregas: guarda o offset lido e só processa
    o que foi acrescentado. A confirmação roda a cada envio e durante turno longo; reler arquivos de
    dezenas de MB a cada vez prendia um núcleo inteiro. Troca de arquivo, truncamento ou reescrita
    (âncora dos bytes antes do offset não bate) zeram o índice e releem do início."""

    def __init__(self, provider: str, min_offset: int = 0, prefix_digest: str | None = None,
                 imported_item_ids: tuple[str, ...] = ()):
        self.provider = provider
        self.min_offset = min_offset
        self.prefix_digest = prefix_digest
        self.boundary_digest = prefix_digest
        self.imported_item_ids = frozenset(imported_item_ids)
        self.lock = threading.Lock()
        self.offset = min_offset
        self.signature = None
        self.anchor = b""
        self.lines: set[str] = set()
        self.pendente: list[str] = []   # fila interna do Claude Code, na ordem do enqueue

    def _zera(self) -> None:
        self.offset = self.min_offset
        self.lines.clear()
        self.pendente.clear()

    def _parser(self):
        # Import local pelo mesmo motivo do merged_history: app.adapters importa app.pqueue no boot.
        if self.provider == "codex":
            from app.adapters.codex.rollout import parse_rollout_obj
            return parse_rollout_obj
        if self.provider in ("pi", "omp"):
            from app.adapters.pi.transcript import parse_obj
            return parse_obj
        if self.provider == "kimi":
            # Kimi: sem o parser proprio, NENHUMA linha do wire casa o shape do Claude (o role mora em
            # `context.append_message`) -> oraculo vazio -> reconcile lia TODA entrega como engolida e
            # redigitava ate max_attempts (medido em producao: 3x "ola", 2026-08-11).
            from app.adapters.kimi.transcript import parse_obj
            return parse_obj
        return None

    def _alimenta_claude(self, obj: dict) -> None:
        etype = obj.get("type")
        # system NAO entra de proposito: recado preso em entrega BLOQUEADA por hook tem
        # preventContinuation=true — o agente nunca o recebeu, e committed_user_lines e o
        # oraculo de "aterrissou na sessao". Conta-lo confirmaria a entrega e a bolha
        # ficaria sem a marca vermelha sobre uma mensagem que nunca chegou (ver parecer
        # G2 rev1, bloqueador 1).
        if etype == "queue-operation":
            c = obj.get("content")
            if not isinstance(c, str):
                # Sem texto, sai a primeira entrada, inclusive quando há pedidos repetidos.
                if obj.get("operation") == "dequeue" and self.pendente:
                    self.pendente.pop(0)
                return
            self.lines.update(_chaves_de_commit(c))
            c = c.strip()
            if obj.get("operation") == "enqueue":
                self.pendente.append(c)
            elif c in self.pendente:
                self.pendente.remove(c)
            return
        # Claude sem terminal: msg orientada no meio do turno aterrissa como
        # `attachment/queued_command`, nunca como `user`. Sem isto a entrega orientada
        # ficava pra sempre sem confirmação e a bolha da fila duplicava a real.
        if etype == "attachment":
            att = obj.get("attachment")
            if isinstance(att, dict) and att.get("type") == "queued_command":
                for b in att.get("prompt") or []:
                    if isinstance(b, dict) and b.get("type") == "text" and isinstance(b.get("text"), str):
                        self.lines.update(_chaves_de_commit(b["text"]))
            return
        if etype != "user":
            return
        content = (obj.get("message") or {}).get("content")
        if isinstance(content, str):
            self.lines.update(_chaves_de_commit(content))
        elif isinstance(content, list):
            for b in content:
                if isinstance(b, dict) and b.get("type") == "text":
                    self.lines.update(_chaves_de_commit(str(b.get("text", ""))))

    def read(self, path: str) -> tuple[set[str], list[str]] | None:
        parse = self._parser()
        with self.lock:
            try:
                with open(path, "rb") as fh:
                    stat = os.fstat(fh.fileno())
                    signature = (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns,
                                 stat.st_ctime_ns)
                    if self.min_offset:
                        from app.conversation_history import verified_prefix, HistoryError
                        if stat.st_size < self.min_offset:
                            raise OSError("transcript truncado antes da importação")
                        try:
                            digest, _ = verified_prefix(path, self.min_offset, self.boundary_digest)
                        except HistoryError as exc:
                            raise OSError("fronteira da importação mudou") from exc
                        self.boundary_digest = digest
                    fh.seek(max(0, self.offset - len(self.anchor)))
                    unchanged = fh.read(len(self.anchor)) == self.anchor
                    if self.signature and (signature[:2] != self.signature[:2]
                            or stat.st_size < self.signature[2]
                            or stat.st_size == self.signature[2] and signature != self.signature
                            or not unchanged):
                        self._zera()
                    fh.seek(self.offset)
                    while fh.tell() < stat.st_size:
                        start = fh.tell()
                        raw = fh.readline(stat.st_size - start)
                        if not raw.endswith(b"\n"):
                            break  # Linha incompleta não comprova entrega nem avança o índice.
                        self.offset = fh.tell()
                        try:
                            obj = json.loads(raw.decode("utf-8", "strict" if self.min_offset else "replace"))
                        except (ValueError, UnicodeError) as exc:
                            if self.min_offset:
                                raise OSError("linha inválida após a importação") from exc
                            continue
                        if not isinstance(obj, dict):
                            if self.min_offset:
                                raise OSError("registro inválido após a importação")
                            continue
                        payload = obj.get("payload") or {}
                        item_id = payload.get("id") if isinstance(payload, dict) else None
                        if item_id in self.imported_item_ids or obj.get("id") in self.imported_item_ids:
                            continue
                        if parse is None:
                            self._alimenta_claude(obj)
                            continue
                        for event in parse(obj):
                            if event.kind == "user_msg" and event.text:
                                self.lines.update(_chaves_de_commit(event.text))
                    # fstat dos dois lados: no Windows o st_ctime_ns do stat pelo caminho nunca
                    # bate com o do handle, e a confirmação falhava sempre.
                    with open(path, "rb") as again:
                        current = os.fstat(again.fileno())
                    if ((current.st_dev, current.st_ino) != signature[:2]
                            or current.st_size < stat.st_size
                            or current.st_size == stat.st_size and (
                                current.st_mtime_ns != stat.st_mtime_ns
                                or current.st_ctime_ns != stat.st_ctime_ns)):
                        raise OSError("transcript mudou durante a confirmação")
                    fh.seek(max(0, self.offset - 256))
                    self.anchor = fh.read(min(self.offset, 256))
                    self.signature = signature
                    return self.lines.copy(), list(self.pendente)
            except OSError as exc:
                self._zera()
                self.signature = None
                self.anchor = b""
                _log.warning("nao deu pra ler o transcript %s pra confirmar entregas: %s", path, exc)
                return None


_indices: dict[tuple[str, str, int], _CommittedIndex] = {}
_indices_lock = threading.Lock()
_INDICES_MAX = 32
_INDICE_CHARS = 1_000_000


def _ler_indice(jsonl: str, provider: str, *, min_offset: int = 0,
                prefix_digest: str | None = None,
                imported_item_ids: tuple[str, ...] = ()) -> tuple[set[str], list[str]] | None:
    if min_offset < 0:
        raise ValueError("fronteira de confirmação inválida")
    chave = (str(Path(jsonl)), provider if provider in ("codex", "pi", "omp", "kimi") else "claude", min_offset)
    with _indices_lock:
        index = _indices.pop(chave, None)
        if (index is None or index.prefix_digest != prefix_digest
                or index.imported_item_ids != frozenset(imported_item_ids)):
            index = _CommittedIndex(chave[1], min_offset, prefix_digest, imported_item_ids)
        _indices[chave] = index
        while len(_indices) > _INDICES_MAX:
            del _indices[next(iter(_indices))]
    lido = index.read(chave[0])
    # Índice grande continua correto, mas não fica retido entre chamadas.
    if lido is None or sum(map(len, lido[0])) > _INDICE_CHARS:
        with _indices_lock:
            if _indices.get(chave) is index:
                del _indices[chave]
    return lido


def committed_user_lines(jsonl: str, provider: str = "claude", *,
                         min_offset: int = 0, prefix_digest: str | None = None,
                         imported_item_ids: tuple[str, ...] = ()) -> set[str] | None:
    """Textos que ATERRISSARAM no transcript (inteiros + por linha), pra confirmar entregas.

    None = NAO DEU PRA LER o transcript. Nunca um set vazio nesse caso, e a diferenca e o bug:
    quem chama isto usa o resultado como oraculo de "chegou na sessao?", e um set vazio responde
    "NADA chegou" — o `reconcile_delivered` entao re-enfileira TODA entrega pendente e o `drain`
    REDIGITA a mensagem do usuario dentro da conversa. Ate 26/08/2026 o `except OSError` aqui
    devolvia o set parcial montado ate o erro, ou seja: falha de leitura autorizava redigitacao.
    No Windows isso nao e hipotetico — ler o .jsonl que o Claude Code esta escrevendo pode voltar
    WinError 32 (arquivo em uso). Oraculo que falhou nao decide nada; ver `_confirm_and_drain`.
    Fontes CRUAS, sem o filtro de meta do parser: (a) entradas `user` — mensagem entregue MID-TURN
    e injetada depois vem embrulhada em meta que o parse_obj descartaria; (b) `queue-operation`
    enqueue — a fila INTERNA do Claude Code registra o texto NO MOMENTO da digitacao, antes de
    virar entrada user. Sem (b), mensagem enfileirada durante um turno longo parecia 'engolida'
    e era REDIGITADA em loop (o bug das mensagens fantasma repetidas).

    provider: o shape do transcript, igual ao merged_history. O Pi poe o role DENTRO de `message`
    (`{"type":"message","message":{"role":"user",...}}`), entao as regras cruas acima nao casam
    NADA e o oraculo devolvia set() vazio -> toda entrega era lida como engolida e o drain
    redigitava o mesmo prompt (double-send medido: pi-e2e.jsonl com attempts: 2). Pi nao tem fila
    interna com `queue-operation`, entao o parser proprio ja basta. Kimi: mesmo motivo, shape
    `context.append_message` — o parser do adapter e usado do mesmo jeito."""
    # Variantes (cru, sem "[Image #N]", sem marcador de anexo, por linha): ver _chaves_de_commit.
    # Falha de leitura devolve None, nunca o set parcial: "estas 40 chegaram e as suas nao" fazia
    # a que faltava ser redigitada.
    lido = _ler_indice(jsonl, provider, min_offset=min_offset, prefix_digest=prefix_digest,
                       imported_item_ids=imported_item_ids)
    return None if lido is None else lido[0]


def linha_mais_parecida(texto: str, committed: set[str]) -> str | None:
    """A linha do transcript mais parecida com `texto`, ou None se nenhuma passa de 60%.

    So roda no caminho de FALHA (o log do requeue). Existe porque `REQUEUE name=X n=1` nao diz
    NADA sobre o porque: o oraculo e comparacao de string, entao o que resolve o proximo caso e o
    diff — um espaco a mais, uma barra invertida comida pelo multiplexador, um prefixo que o
    harness prependou. Sem isso a proxima ocorrencia custa a mesma leitura de codigo desta.
    """
    if not texto or not committed:
        return None
    perto = difflib.get_close_matches(texto, list(committed), n=1, cutoff=0.6)
    return perto[0] if perto else None


def _queue_method(method):
    signature = inspect.signature(method)

    @functools.wraps(method)
    def routed(self, *args, **kwargs):
        from app.runtime_queue import route_queue
        bound = signature.bind(self, *args, **kwargs)
        bound.apply_defaults()
        managed, result = route_queue(self, method.__name__, {k: v for k, v in bound.arguments.items() if k != "self"})
        if managed:
            return result
        return method(self, *args, **kwargs)
    return routed


class PromptQueue:
    """Fila duravel de prompts por sessao (sidecar JSONL). Registra cada envio pra que msgs
    enfileiradas (mandadas com o Claude trabalhando) — que o Claude Code NEM sempre grava no
    proprio transcript — aparecam no fluxo, em ordem, e sobrevivam a reload. O merge dedup-a
    contra o transcript: quando o Claude Code grava o prompt real, a entrada da fila some."""

    def __init__(self, name: str):
        self.name = name
        self.path = _queue_dir() / f"{_sanitize(name)}.jsonl"

    @_queue_method
    def _write_atomic(self, rows: list[dict]) -> None:
        # Escrita atomica (tmp + replace) pra um reader nunca pegar o arquivo pela metade.
        tmp = self.path.with_suffix(".jsonl.tmp")
        # dumps_safe (nao json.dumps): surrogate solto no texto do usuario passa pelo json.dumps e
        # so estoura no encode do write_text -> o POST /input inteiro virava 500 e a msg sumia.
        tmp.write_text("".join(dumps_safe(r) + "\n" for r in rows), encoding="utf-8")
        atomico.substituir(tmp, self.path)

    @_queue_method
    def append(self, text: str, delivered: bool = False, ts: float | None = None,
               pre_transcript: bool = False) -> dict:
        # delivered=False por padrao = enfileirada mas NAO digitada na TUI (o /input passa True quando
        # o send_prompt realmente digitou). So entradas False sao drenadas -> sem isto um upgrade
        # re-enviaria toda entrada legada (= double-send em massa).
        # ts = QUANDO O USUARIO MANDOU, nao quando este append rodou: quem envia primeiro digita na
        # TUI (e o Claude Code ja grava o prompt no transcript) e so depois chama este append — o
        # default time.time() cairia DEPOIS do commit e o dedup ts-aware do merged_history leria o
        # proprio commit como "anterior, de outra msg igual" -> msg duplicada no historico. Quem tem
        # send antes do append passa o ts capturado ANTES do send (ver api._send_one).
        # scrub AQUI tambem (nao so no _write_atomic): a entrada devolvida ao caller tem que ser a
        # MESMA que foi pro disco, senao o reconcile/dedup compararia o texto cru contra o transcript
        # (que ja recebeu o U+FFFD) e nunca casaria.
        # pre_transcript: entrada carimbada ANTES de a sessão existir (passagem de bastão) — isenta
        # do corte por idade em toda a fila. Ver _da_sessao_atual; só grava a chave quando é True
        # pra não engordar toda entrada normal com um campo que nunca será lido.
        entry = {"id": uuid.uuid4().hex, "text": scrub_surrogates(text),
                 "ts": time.time() if ts is None else ts, "delivered": delivered}
        if pre_transcript:
            entry["pre_transcript"] = True
        # ponytail: lock global serializa o read-modify-write; 2 POSTs /input concorrentes (handlers
        # sync no threadpool) senao liam as mesmas rows e um sobrescrevia o outro (entrada perdida).
        # upgrade: lock per-path se o throughput de uma sessao virar gargalo.
        with _append_lock:
            rows = self.load()
            rows.append(entry)
            if len(rows) > _MAX_ENTRIES:
                rows = rows[-_MAX_ENTRIES:]
            self._write_atomic(rows)
        return entry

    @_queue_method
    def append_saida_local(self, text: str, confirms: str | None = None) -> dict:
        """Texto do AGENTE que nao entra no transcript (ver _saida_local). Entregue e confirmada
        de nascenca: nunca e drenada, redigitada nem reconciliada. `confirms`: o comando que a CLI
        respondeu sozinha; a entrada mais antiga com esse texto fica confirmada por esta resposta."""
        entry = {"id": uuid.uuid4().hex, "text": scrub_surrogates(text), "ts": time.time(),
                 "delivered": True, "confirmed": True, "papel": "assistant"}
        with _append_lock:
            rows = self.load()
            comando = confirms.strip() if isinstance(confirms, str) else ""
            if comando.startswith("/") and len(comando) > 1:
                alvo = next((r for r in rows if not r.get("confirmed") and r.get("papel") != "assistant"
                             and r.get("delivered") and isinstance(r.get("text"), str)
                             and r["text"].strip() == comando), None)
                if alvo is not None:
                    alvo["confirmed"] = True
                    alvo.pop("desistiu", None)
            rows.append(entry)
            if len(rows) > _MAX_ENTRIES:
                rows = rows[-_MAX_ENTRIES:]
            self._write_atomic(rows)
        return entry

    @_queue_method
    def claim_undelivered(self, min_ts: float = 0.0, limit: int | None = None,
                          *, entry_id: str | None = None) -> list[dict]:
        """Reivindica (atomicamente) entradas ainda nao entregues: vira delivered=True e devolve as
        reivindicadas. Sob _append_lock -> com N drains concorrentes so UM pega cada entrada (os
        outros pegam []) = single-flight, sem double-send. `is False` ESTRITO: legada (sem a chave) ou
        ja entregue NAO entra. min_ts poda entradas de sessao antiga (pre-/clear)."""
        with _append_lock:
            rows = self.load()
            claimed = []
            for r in rows:
                if entry_id is not None and str(r.get("id")) != entry_id:
                    continue
                if r.get("delivered") is False and _da_sessao_atual(r, min_ts):
                    r["delivered"] = True
                    claimed.append(dict(r))
                    if limit is not None and len(claimed) >= limit:
                        break
            if claimed:
                self._write_atomic(rows)
            return claimed

    @_queue_method
    def set_delivered(self, entry_id: str, value: bool, *, steered: bool = False) -> None:
        """Marca UMA entrada (por id) como delivered=value e reescreve atomico. Usado pra reverter um
        claim quando o envio nao chegou a tocar a TUI (provadamente pre-envio)."""
        with _append_lock:
            rows = self.load()
            for r in rows:
                if str(r.get("id")) == entry_id:
                    r["delivered"] = value
                    if value and steered:
                        r["steered"] = True
                    elif not value:
                        r.pop("steered", None)
                    break
            else:
                return
            self._write_atomic(rows)

    @_queue_method
    def desistir(self, entry_id: str) -> None:
        """Marca UMA entrada reivindicada como perdida sem passar pelo reconcile: o envio nem
        pôde ser tentado (sessão que não sobe). Mesmos campos do desfecho `desistiu` de lá."""
        with _append_lock:
            rows = self.load()
            for r in rows:
                if str(r.get("id")) == entry_id:
                    r["delivered"] = True
                    r["desistiu"] = True
                    r["desistiu_ts"] = time.time()
                    self._write_atomic(rows)
                    return

    @_queue_method
    def bump_attempts(self, entry_id: str) -> int:
        """Incrementa `attempts` de UMA entrada e devolve o novo total (0 = entrada nao existe).

        Existe pro requeue do `drain` ter TETO: reverter delivered=False sem contar tentativa deixa
        uma entrada que falha sempre girando pra sempre no executor de envio. Mesmo campo que o
        reconcile ja usa (`reconcile_delivered`, max_attempts=2).
        """
        with _append_lock:
            rows = self.load()
            for r in rows:
                if str(r.get("id")) == entry_id:
                    r["attempts"] = int(r.get("attempts") or 0) + 1
                    self._write_atomic(rows)
                    return int(r["attempts"])
        return 0

    @_queue_method
    def entry_delivered(self, entry_id: str) -> bool | None:
        """delivered? de UMA entrada por id. None = entrada nao existe (prunada/sumiu com /clear).
        Ancora do loop runner: so tica depois do goal constar entregue na TUI."""
        if not entry_id:
            return None
        for r in self.load():
            if str(r.get("id")) == entry_id:
                return bool(r.get("delivered"))
        return None

    @_queue_method
    def confirm_delivered(self, apenas: Callable[[dict], bool] | None = None) -> int:
        """Carimba `confirmed` em TODA entrada delivered ainda não confirmada. Devolve quantas.

        Quem chama é o steer do Kimi (POST /steer): o ctrl-s promove a fila INTERNA da TUI pro
        turno em curso, e essa fila é exatamente o conjunto delivered-não-confirmado daqui —
        digitado, mas ainda fora do wire, porque o Kimi só grava o append_message da msg steerada
        no FIM do turno (medido em 19/08/2026: 34s depois do ctrl-s). Sem este carimbo a bolha
        "na fila" ficava acesa o turno inteiro sobre uma mensagem que já estava no turno.

        `apenas`: restringe às entradas que o predicado aceita (o comando local da sessão sem
        terminal confirma só slash, nunca um prompt comum de carona).
        """
        with _append_lock:
            rows = self.load()
            n = 0
            for r in rows:
                if r.get("delivered") is True and not r.get("confirmed") and (apenas is None or apenas(r)):
                    r["confirmed"] = True
                    n += 1
            if n:
                self._write_atomic(rows)
            return n

    @_queue_method
    def prune_before(self, min_ts: float) -> int:
        # Entradas de sessao ANTERIOR (ts < inicio do transcript atual) nunca mais casam nem drenam
        # — so acumulavam lixo e mantinham o cheap-check do drain quente pra sempre. Remove.
        # Devolve QUANTAS cairam: a poda apaga a bubble do chat junto, entao sumir calado esconderia
        # mensagem descartada — quem chama loga (falha aparece, nao some).
        if min_ts <= 0:
            return 0
        with _append_lock:
            rows = self.load()
            kept = [r for r in rows if _da_sessao_atual(r, min_ts)]
            if len(kept) != len(rows):
                self._write_atomic(kept)
            return len(rows) - len(kept)

    @_queue_method
    def reconcile_delivered(self, committed: set[str], min_ts: float, now: float,
                            grace: float = 8.0, max_attempts: int = 2,
                            confirm_only: bool = False,
                            na_fila_tui: set[str] | frozenset[str] = frozenset()) -> list[dict]:
        """Confirma entregas contra o transcript ou RE-ENFILEIRA as engolidas. delivered=True quer
        dizer 'send_keys chamado', nao 'Claude recebeu' — a TUI pode engolir as teclas (redraw) e a
        msg sumia com cara de entregue. Entrada delivered, nao-confirmada, da sessao atual e mais
        velha que `grace`: texto no transcript -> confirmed=True (para de checar); ausente ->
        delivered=False + attempts+1 (o drain reentrega); attempts >= max_attempts -> desiste
        (`desistiu=True`: fica VISIVEL como bubble = comportamento antigo, sem loop de redigitacao).
        Os dois desfechos sao campos DIFERENTES de proposito — `confirmed` esconde o eco, `desistiu`
        nao. Ate 2026-08-11 os dois gravavam `confirmed` e a msg engolida sumia da tela.
        Devolve as re-enfileiradas.

        `confirm_only` (usado no MEIO do turno, sessao `working`): so carimba o que esta
        comprovado no transcript — texto casou, ou sessao anterior (min_ts). Ausente do transcript
        NAO decide nada (nem desistiu, nem requeue): o texto pode ainda estar na fila interna da
        TUI, e o desistiu viraria aviso falso de "nao chegou" sobre msg que chega depois. Quem
        chama reagenda a checagem enquanto sobrar pendente."""
        with _append_lock:
            rows = self.load()
            requeued: list[dict] = []
            descartadas: list[dict] = []
            changed = False
            # Cada linha do transcript so pode confirmar UMA entrada. `committed` e um set, entao
            # duas entradas com o MESMO texto (comum em resposta de picker: "Respondendo a pergunta:
            # Sim" se repete) casariam as duas contra a mesma linha — e uma que se perdeu de verdade
            # viraria `confirmed`, escondendo a falha em vez de mostra-la. Gasta-se a linha ao usar.
            disponiveis = set(committed)
            # Linhas que casam EXATO com alguma entrada pendente pertencem a ELA — o prefixo de
            # outra entrada nao pode leva-las (senao "pode seguir" comeria a linha de "pode seguir
            # com a Task 4 agora": a perdida vira entregue e a que chegou ganha a marca — o defeito
            # da Task reaberto por outra porta, parecer G2 rev1 bloqueador 2).
            reservadas = frozenset().union(*[
                _linhas_da_entrada(r) & disponiveis
                for r in rows if r.get("delivered") is True and not r.get("confirmed")
            ] or [frozenset()])
            # Linha com sufixo (nenhuma entrada casa exato nelas) -> pertence ao MAIOR prefixo que
            # a reivindica; sem isto a entrada mais curta levava a linha da mais especifica e as
            # duas marcas invertiam (parecer G2 rev2, bloqueador 1).
            dono = _dono_do_prefixo(rows, disponiveis)
            for r in rows:
                if r.get("delivered") is not True or r.get("confirmed"):
                    continue
                ts = float(r.get("ts") or 0.0)
                if r.get("desistiu"):
                    # RESGATE: a msg apareceu no transcript DEPOIS de a gente desistir dela. Sem
                    # isto, `desistiu` era irreversivel e a bolha ficava avisando "nao chegou"
                    # eternamente sobre uma msg que CHEGOU — mentira que so o reload escondia (o
                    # merged_history a absorve; o SSE ao vivo, nao). Medido em 13/08/2026 numa
                    # sessao Kimi: 6 de 7 desistidas estavam no wire.jsonl no fim do dia.
                    #
                    # `ts < min_ts` NAO resgata: entrada de uma sessao ANTERIOR (pre-/clear) seria
                    # comparada contra o transcript de AGORA, e um texto curto e repetido ("Sim",
                    # "1") casaria por coincidencia — dando por entregue o que nunca chegou.
                    if not _da_sessao_atual(r, min_ts):
                        continue
                    # REENVIO: o usuario mandou o mesmo texto DEPOIS de esta desistir. A linha do
                    # transcript e da entrada nova (uma linha, uma entrada), entao esta nunca seria
                    # resgatada e a bolha "nao chegou — reenvie" ficava pra sempre ao lado da
                    # mensagem que chegou. Sai da fila. O criterio e o INSTANTE da desistencia,
                    # nao so o texto: duas respostas iguais mandadas antes de qualquer desistencia
                    # (dois "Sim" de picker, um perdido) continuam separadas, e a perdida segue
                    # visivel — o teste das duas entradas iguais cobra isso.
                    if _tem_reenvio(r, rows):
                        descartadas.append(r)
                        continue
                    linhas_r = _linhas_da_entrada(r)
                    casou = _casam(linhas_r, disponiveis, reservadas, dono)
                    if casou:
                        disponiveis -= casou    # a(s) linha(s) foi usada: nao confirma outra entrada
                        r["confirmed"] = True
                        r.pop("desistiu", None)
                        changed = True
                    continue
                if not _da_sessao_atual(r, min_ts):
                    r["confirmed"] = True   # sessao anterior: fora do escopo (e silencia o check)
                    changed = True
                    continue
                if now - ts < grace:
                    continue                # recente demais: o transcript pode nao ter gravado ainda
                # Compara CRU e podado (espelha o lado do committed_user_lines): so um dos lados
                # podado deixava msg com anexo orfa -> requeue indevido.
                text_raw = str(r.get("text") or "").strip()
                lines = _linhas_da_entrada(r)
                if lines & na_fila_tui:
                    # Entregue e AINDA na fila interna da TUI: nao confirma (a bolha real so nasce
                    # no consumo) nem redigita/desiste (chegou). Fica visivel como bolha da fila.
                    continue
                cons = _casam(lines, disponiveis, reservadas, dono)
                if not text_raw or cons:
                    disponiveis -= cons            # as linhas casadas confirmam UMA entrada so
                    r["confirmed"] = True
                elif confirm_only or r.get("steered") is True:
                    # RPC confirmado não é uma tecla possivelmente engolida: não redigitar.
                    # Meio do turno sem prova: nao decide. Entrada segue entregue/nao-confirmada e
                    # o caller reagenda — quando a sessao ficar ociosa, o caminho normal desiste ou
                    # re-enfileira com tentativa contada.
                    continue
                elif int(r.get("attempts") or 0) >= max_attempts:
                    # DESISTIU != CONFIRMADA. `confirmed` quer dizer "o texto esta comprovadamente
                    # no transcript" e e o que faz merged_history/follow ESCONDEREM o eco (a bolha
                    # real ja cobre). Aqui nao ha bolha real nenhuma — a msg foi engolida de vez —,
                    # entao marcar confirmed sumia com a mensagem do usuario: some do /history e do
                    # SSE, e o `pending` do front so a segura ate o proximo reload. Campo proprio:
                    # para de rechecar (o loop acima pula) SEM esconder.
                    r["desistiu"] = True
                    r["desistiu_ts"] = now   # e o que separa "reenviou depois" de "mandou 2x"
                else:
                    r["delivered"] = False
                    r["attempts"] = int(r.get("attempts") or 0) + 1
                    requeued.append(dict(r))
                changed = True
            if descartadas:
                # Rastro obrigatorio: e o unico caminho em que uma entrada perdida sai da tela sem
                # confirmacao propria — um "Sim" desistido pode ser descartado por um "Sim" de OUTRA
                # pergunta horas depois. Sem esta linha a perda sumia sem vestigio.
                for d in descartadas:
                    _log.info("fila %s: entrada desistida %s descartada por reenvio do mesmo texto (%d chars)",
                              self.path.stem, d.get("id"), len(str(d.get("text") or "")))
                rows = [r for r in rows if not any(r is d for d in descartadas)]
                changed = True
            if changed:
                self._write_atomic(rows)
            return requeued

    @_queue_method
    def remove(self, entry_id: str) -> bool:
        """Tira UMA entrada DESISTIDA da fila pelo id (o botao "descartar" da bolha perdida).
        True = existia e saiu. Entrada ainda por entregar ou ja confirmada nao sai por aqui: o id
        vem do cliente, e um id velho nao pode derrubar mensagem que ainda vai ser digitada."""
        with _append_lock:
            rows = self.load()
            restantes = [r for r in rows if not (r.get("id") == entry_id and r.get("desistiu"))]
            if len(restantes) == len(rows):
                return False
            self._write_atomic(restantes)
            return True

    @_queue_method
    def clear(self) -> None:
        # Remove o sidecar inteiro. Usado quando /clear reinicia a sessao do Claude Code: as entradas
        # pertencem ao transcript ANTIGO e nao devem reaparecer como bubble no transcript novo (a fila
        # e keyed pelo NOME da sessao, que sobrevive ao /clear -> sem isto, viram fantasma).
        self.path.unlink(missing_ok=True)
        self.path.with_suffix(".jsonl.tmp").unlink(missing_ok=True)

    @_queue_method
    def rename(self, new_name: str) -> None:
        # Move o sidecar pro nome novo, preservando entradas nao-drenadas (a fila e keyed pelo NOME;
        # sem mover, a sessao renomeada perderia a fila e ela viraria orfa no nome velho). Move atomico
        # (mesmo dir). Sem fila = no-op. O .tmp meio-escrito nao migra.
        self.path.with_suffix(".jsonl.tmp").unlink(missing_ok=True)
        if self.path.exists():
            atomico.substituir(self.path, _queue_dir() / f"{_sanitize(new_name)}.jsonl")

    @_queue_method
    def load(self) -> list[dict]:
        if not self.path.exists():
            return []
        out = []
        for line in self.path.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except (json.JSONDecodeError, ValueError):
                continue
        return out

    async def follow(self, min_ts: float = 0.0, emit_confirmed: bool = False) -> AsyncIterator[ChatEvent]:
        # Emite as entradas existentes e depois vigia novos appends, como user_msg sintetico.
        # Guarda os estados já vistos (o append reescreve o arquivo inteiro -> rastrear posicao
        # quebraria; reload + dedup por id e simples e correto). min_ts: descarta entradas anteriores
        # ao inicio da sessao atual (ex: pre-/clear) — espelha a poda do merged_history no live SSE.
        # Entrega e desistência mudam depois do primeiro evento. O front substitui pelo mesmo id.
        seen: dict[str, tuple[bool | None, bool | None, bool]] = {}
        inicio: float = time.time()
        primeira: bool = True
        last_marker = object()

        def emit_new() -> list[ChatEvent]:
            nonlocal last_marker
            try:
                stat = self.path.stat()
            except FileNotFoundError:
                marker = None
            else:
                marker = (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns)
            if marker == last_marker:
                return []
            evs = []
            for entry in self.load():
                eid = str(entry.get("id"))
                if not eid:
                    continue
                # Codex recebe a baixa explícita; os demais mantêm a supressão do eco.
                confirmed = bool(entry.get("confirmed")) and not _saida_local(entry)
                if confirmed and not emit_confirmed:
                    continue
                event = _entry_event(entry)
                if confirmed:
                    event.queued_confirmed = True
                signature = (event.desistiu, event.queued_delivered, confirmed)
                if eid in seen and seen[eid] == signature:
                    continue
                seen[eid] = signature
                if (primeira and _saida_local(entry)
                        and float(entry.get("ts") or 0) < inicio - _FOLGA_SAIDA_LOCAL_S):
                    # Nota local antiga já está no /history, no lugar do relógio dela. Emitida aqui,
                    # o front a anexava no FIM da conversa a cada vez que o chat abria. Reconexão
                    # depois de queda longa: o front refaz a cauda do /history (Chat.svelte).
                    continue
                if min_ts and not _da_sessao_atual(entry, min_ts):
                    continue
                evs.append(event)
            # O marcador anterior à leitura evita perder uma gravação concorrente.
            last_marker = marker
            return evs

        # emit_new() faz read_text do sidecar -> roda no threadpool pra nao bloquear o loop. As chamadas
        # sao sequenciais (uma await por vez), entao o set `seen` que ela muta nao corre risco de corrida.
        for ev in await asyncio.to_thread(emit_new):
            yield ev
        primeira = False
        # yield_on_timeout: cobre entrada gravada entre o emit_new acima e o watcher armar (senao so
        # apareceria no proximo write da fila). O dir e COMPARTILHADO por todas as sessoes -> filtra:
        # so recarrega quando o toque e no NOSSO arquivo (ou no timeout do heartbeat).
        async for changes in awatch(self.path.parent, yield_on_timeout=True, rust_timeout=5000,
                                    recursive=False):
            if changes and not any(Path(p).name == self.path.name for _, p in changes):
                continue
            for ev in await asyncio.to_thread(emit_new):
                yield ev


# Janela inicial do tail-read do /history com limit (board/hover): parsear so o fim do arquivo em
# vez do jsonl inteiro (10-50MB em sessao longa). Cresce 4x ate render >= limit eventos ou alcancar
# o inicio do arquivo.
_TAIL_WINDOW = 256 * 1024


def _tail_offset(path: str, window: int) -> int:
    # Offset da 1a linha COMPLETA dentro dos ultimos `window` bytes. 0 = "parseia do inicio", que
    # tambem e o fallback se o arquivo sumir no meio (rename/clear): _parse_from trata o proprio
    # OSError e devolve historico vazio, igual ao caminho sem limit.
    try:
        size = os.path.getsize(path)
        if size <= window:
            return 0
        with open(path, "rb") as fh:
            fh.seek(size - window)
            fh.readline()  # descarta a linha partida no corte
            return fh.tell()
    except OSError:
        return 0


def merged_history(name: str, jsonl: str, provider: str = "claude",
                   limit: int | None = None) -> list[ChatEvent]:
    """Historico = eventos do transcript + entradas da fila ainda NAO absorvidas pelo transcript,
    ordenado por timestamp. Dedup TS-AWARE: descarta entrada da fila cujo texto ja apareca (por
    linha) num user_msg commitado DEPOIS dela (o transcript grava o prompt apos a entrega). Match
    por texto sozinho engolia repeticao: o 2o "ok" enfileirado sumia por causa do 1o ja commitado.
    Entradas sem timestamp herdam o ts anterior (carry-forward) pra manter a ordem do arquivo.

    provider: qual parser usar pra `jsonl` -- Claude, Codex e Pi tem shapes diferentes (ver
    app.adapters.codex.rollout e app.adapters.pi.transcript). _ts_of_obj cai pro `timestamp`
    (epoch ms) dentro de `message` quando nao ha ISO no topo da linha -- o caso do Pi. Import
    local dos parsers de Codex/Pi evita ciclo (app.adapters importa app.pqueue no boot, pra
    PromptQueue).

    limit: quando dado, parseia so a CAUDA do arquivo (tail-read reverso, janela que cresce ate
    render >= limit eventos) em vez do jsonl inteiro -- e o que torna o burst de /history do board
    (dezenas de cards x transcripts de MB) barato. O caller ainda corta evs[-limit:]; aqui limit so
    dimensiona a janela. Trade-off aceito (ponytail): committed_ts enxerga so a janela, entao
    entrada de fila NAO-confirmada cujo commit ficou fora dela reapareceria como pendente -- na
    pratica o reconcile marca `confirmed` e a entrada nem chega aqui."""
    from app.conversation_history import session_transfer, composed_history
    record = session_transfer(name, jsonl, provider)
    if record:
        return composed_history(name, jsonl, provider, record, limit)
    _pi_stream = None
    if provider == "codex":
        from app.adapters.codex.rollout import parse_rollout_obj as _parse
    elif provider in ("pi", "omp"):
        # Stream (com memoria de uma linha) e nao parse_obj solto: e o que tira o contexto de hook
        # colado no inicio da mensagem do usuario. Uma instancia por _parse_from — a janela do
        # tail-read cresce e re-parseia, e um estado carregado da tentativa anterior soltaria
        # mensagem retida no lugar errado.
        from app.adapters.pi.transcript import Stream as _pi_stream
        _parse = parse_obj      # trocado dentro do _parse_from pela instancia da vez
    elif provider == "kimi":
        # parse_obj solto basta: o parser do wire do Kimi nao guarda estado entre linhas.
        from app.adapters.kimi.transcript import parse_obj as _parse
    elif provider == "orq":
        from functools import partial
        from app.adapters.orq.adapter import parse_obj as orq_parse_obj
        _parse = partial(orq_parse_obj, run_dir=Path(jsonl).parent)
    else:
        _parse = parse_obj
    items: list[tuple[float, int, ChatEvent]] = []
    committed_ts: dict[str, float] = {}  # linha normalizada -> maior ts em que commitou
    prev_ts = 0.0
    start_ts = 0.0  # 1o ts real do transcript = inicio da sessao atual (pra podar fila pre-/clear)

    def _parse_from(offset: int) -> None:
        # Preenche items/committed_ts do offset ao fim. Zera acumuladores: a janela do tail-read
        # pode crescer e re-parsear. Itera linha-a-linha (nao carrega o transcript inteiro em RAM)
        # e parseia o JSON UMA vez por linha.
        nonlocal prev_ts, start_ts
        items.clear()
        committed_ts.clear()
        prev_ts = 0.0
        # Tail: o inicio da sessao esta FORA da janela -> _transcript_start_ts le do comeco do
        # arquivo ate o 1o ts (early return; sem cap de linhas — header longo sem timestamp
        # inflaria o start_ts e podaria fila valida).
        start_ts = (_transcript_start_ts(jsonl) or 0.0) if offset else 0.0
        try:
            fh = open(jsonl, encoding="utf-8", errors="replace")
        except OSError:
            return  # sessao nova: jsonl ainda nao existe -> historico vazio (limpo), nao 500
        stream = _pi_stream() if _pi_stream else None
        parse = stream.feed_events if stream else _parse
        # Reescrita do `--resume` (ver transcript.RewriteFilter): so o Claude regrava o jsonl.
        reescrita = RewriteFilter() if provider == "claude" else None

        barrados: set[str] = set()

        def _absorve(ts: float, i: int, evs: list[ChatEvent]) -> None:
            for ev in evs:
                # Cada reenvio de um prompt barrado grava outra entrada com o MESMO id "held:". O
                # SSE junta por id; a lista do historico nao, e id repetido quebra o {#each} do front.
                if ev.id.startswith("held:"):
                    if ev.id in barrados:
                        continue
                    barrados.add(ev.id)
                # ts do proprio evento quando ha (o parser do Pi preenche): um user_msg retido sai
                # junto com a linha seguinte, e herdar o ts DELA o tiraria de ordem.
                ets = ev.ts or ts
                items.append((ets, i, ev))
                if ev.kind == "user_msg" and ev.text:
                    for ln in _chaves_de_commit(ev.text):
                        if ets > committed_ts.get(ln, 0.0):
                            committed_ts[ln] = ets

        i = 0
        with fh:
            if offset:
                fh.seek(offset)
            for i, line in enumerate(fh):
                # Anexo que nao vira bolha: so o relogio dele conta. RewriteFilter.keep ignora anexo.
                if reescrita is not None and (att_ts := silent_attachment_timestamp(line)) is not None:
                    obj, evs = {"timestamp": att_ts}, []
                else:
                    try:
                        obj = json.loads(line)
                    except (json.JSONDecodeError, ValueError):
                        continue
                    if reescrita is not None and not reescrita.keep(obj):
                        continue
                    evs = parse(obj)
                # ts ANTES do `continue`: com o parser do Pi a 1a linha util e um user_msg que fica
                # RETIDO (devolve [] nela), e pular o relogio aqui empurrava o start_ts pra linha
                # seguinte que solta algo — a resposta do assistente, minutos depois. Efeito: toda
                # entrada de fila enfileirada nesse intervalo caia na poda de "anterior ao inicio da
                # sessao" e sumia do historico, calada. O start_ts e o INICIO da sessao, entao a 1a
                # linha com relogio manda, tenha ela virado bolha ou nao.
                line_ts = _ts_of_obj(obj)
                if line_ts > 0:
                    if start_ts == 0.0:
                        start_ts = line_ts
                    prev_ts = line_ts
                if not evs:
                    continue
                ts = line_ts or prev_ts
                prev_ts = ts
                _absorve(ts, i, evs)
            if stream:
                _absorve(prev_ts, i + 1, stream.flush_events())

    if limit is not None and limit > 0:
        window = _TAIL_WINDOW
        while True:
            off = _tail_offset(jsonl, window)
            _parse_from(off)
            if off == 0 or len(items) >= limit:
                break
            window *= 4
    else:
        _parse_from(0)

    return merge_current_queue(name, items, committed_ts, prev_ts, start_ts)


def merge_current_queue(name: str, items: list[tuple[float, int, ChatEvent]],
                        committed_ts: dict[str, float], prev_ts: float,
                        start_ts: float) -> list[ChatEvent]:
    """Reconcilia uma fila com commits apenas da fonte ativa."""
    # Entradas da fila entram com tiebreaker alto -> caem DEPOIS de eventos do transcript de mesmo ts.
    for entry in PromptQueue(name).load():
        text = (entry.get("text") or "").strip()
        if not text:
            continue
        ts = float(entry.get("ts") or prev_ts)
        if _saida_local(entry):
            if not start_ts or _da_sessao_atual(entry, start_ts, ts):
                items.append((ts, 10**9, _entry_event(entry)))
            continue
        if entry.get("confirmed"):
            continue  # comprovadamente no transcript (reconcile) -> a bolha real ja cobre
        # Absorvida so se o texto commitou DEPOIS de enfileirada. O ts da entrada e o do INSTANTE DO
        # ENVIO (carimbado antes do send — ver append/api._send_one), nao o do append; por isso o
        # write do transcript e sempre >= ele e o commit da propria msg casa. Antes o ts saia do
        # append, que roda DEPOIS do send: caia ~ms apos o commit e a msg duplicava no historico.
        # Commit ANTERIOR ao envio e de outra msg igual -> esta segue pendente (ex: 2o "ok").
        # A entrada tambem e absorvida pela LEGENDA (texto sem o "📎 imagem: <path>"): com anexo o
        # transcript nunca guarda o texto identico ao que a fila digitou — ver _chaves_de_commit.
        cap = _strip_attach(text).strip()
        if max(committed_ts.get(text, -1.0),
               committed_ts.get(cap, -1.0) if cap else -1.0) >= ts:
            continue
        # Poda: entrada anterior ao inicio da sessao atual e de uma sessao antiga (ex: pre-/clear, que
        # cria transcript novo). Sem isto, nunca casaria com o transcript novo e viraria fantasma.
        # `ts` (com carry-forward) e não o campo cru: entrada legada/editada à mão, sem `ts`, é
        # ordenada aqui pelo relógio da linha anterior — cortá-la por 0.0 a fazia sumir do
        # histórico em silêncio, que é o oposto do que a fila durável existe pra garantir.
        if start_ts and not _da_sessao_atual(entry, start_ts, ts):
            continue
        items.append((ts, 10**9, _entry_event(entry)))

    items.sort(key=lambda x: (x[0], x[1]))
    return [ev for _, _, ev in items]


def _marca_do_codigo() -> int:
    # Os mesmos bytes em disco rendem outra resposta quando o PARSER muda. Sem isto o 304 segura
    # no cliente o historico lido pelo codigo antigo ate a conversa andar de novo.
    try:
        return max(p.stat().st_mtime_ns for p in Path(__file__).parent.rglob("*.py"))
    except (OSError, ValueError):
        return 0


_CODIGO = _marca_do_codigo()


def historico_etag(name: str, jsonl: str, provider: str, limit: int | None) -> str | None:
    """Validador do que `merged_history` devolveria, por METADADO -- sem ler os arquivos.

    Cobre as DUAS fontes que ela funde (transcript e sidecar da fila) porque mensagem enfileirada
    aparece no historico antes de estar no jsonl. `provider` e `limit` entram porque mudam a
    resposta com os mesmos bytes em disco (parser diferente, cauda de tamanho diferente).

    Granularidade: tamanho + mtime_ns. Os dois arquivos so crescem por append, entao escrita que
    nao mexe no tamanho nem no relogio nao existe aqui. Sem o transcript nao ha validador (None):
    melhor sempre baixar do que servir 304 sobre um arquivo que nem da pra medir.
    """
    from app.conversation_history import session_transfer, composition_etag
    record = session_transfer(name, jsonl, provider)
    if record:
        return composition_etag(name, jsonl, provider, record, limit, _CODIGO)

    def marca(p: str | Path) -> str:
        try:
            st = os.stat(p)
        except OSError:
            return "-"
        return f"{st.st_size}.{st.st_mtime_ns}"

    t = marca(jsonl)
    if t == "-":
        return None
    return f'"{t}-{marca(_queue_dir() / f"{_sanitize(name)}.jsonl")}-{provider}-{limit}-{_CODIGO}"'
