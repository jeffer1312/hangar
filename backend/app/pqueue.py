import asyncio
import json
import re
import threading
import time
import uuid
from datetime import datetime
from pathlib import Path
from typing import AsyncIterator

from watchfiles import awatch

from app.config import settings
from app.models import ChatEvent
from app.transcript import parse_obj

# Limite de entradas mantidas no sidecar (poda no append pra nao crescer sem fim).
_MAX_ENTRIES = 1000


def _queue_dir() -> Path:
    # Sidecar FORA do transcript do Claude Code (nunca toca no arquivo dele). Fica ao lado de
    # projects/, no diretorio de config (~/.claude-work por padrao).
    d = Path(settings.projects_dir).parent / ".claude-pocket-queue"
    d.mkdir(parents=True, exist_ok=True)
    return d


_append_lock = threading.Lock()  # serializa o read-modify-write do append (handlers sync no threadpool)


def _sanitize(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "-", name)


def _entry_event(entry: dict) -> ChatEvent:
    # user_msg sintetico com id prefixado ("queued-") pro front distinguir de evento real do
    # transcript. ts fica None de proposito: o ts so serve pra ORDENAR no historico, nao pra
    # exibir (senao bubble enfileirada mostraria hora e as do transcript nao -> inconsistente).
    return ChatEvent(kind="user_msg", id="queued-" + str(entry.get("id")), text=entry.get("text"))


def _ts_of_obj(obj: dict) -> float:
    # Epoch (s) do campo `timestamp` (ISO 8601 com Z) de uma entrada do transcript; 0.0 se ausente.
    t = obj.get("timestamp")
    if not isinstance(t, str):
        return 0.0
    try:
        return datetime.fromisoformat(t.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return 0.0


def _ts_of_line(line: str) -> float:
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return 0.0
    return _ts_of_obj(obj)


def _transcript_start_ts(jsonl: str) -> float:
    # ts (epoch) da 1a linha COM timestamp do transcript = inicio da sessao atual. Toda entrada da
    # fila mais antiga que isto pertence a uma sessao anterior (ex: pre-/clear, que cria transcript
    # novo com novo session-id) e nao deve reaparecer como bubble. Le so ate achar o 1o ts (early
    # return) pra nao varrer transcript gigante. 0.0 se nao houver ts -> sem poda (fallback seguro).
    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                ts = _ts_of_line(line)
                if ts > 0:
                    return ts
    except OSError:
        pass
    return 0.0


# Marcador de anexo no texto do app ("legenda — 📎 imagem: <path>"): o transcript grava SO a
# legenda -> tirar antes de casar com o transcript (senao msg com anexo nunca confirmaria).
_ATTACH_RE = re.compile(r"(?:\s*—\s*)?📎\s*(?:imagem|arquivo):.*$", re.S)


def _strip_attach(text: str) -> str:
    return _ATTACH_RE.sub("", text)


def committed_user_lines(jsonl: str) -> set[str]:
    """Textos que ATERRISSARAM no transcript (inteiros + por linha), pra confirmar entregas.
    Fontes CRUAS, sem o filtro de meta do parser: (a) entradas `user` — mensagem entregue MID-TURN
    e injetada depois vem embrulhada em meta que o parse_obj descartaria; (b) `queue-operation`
    enqueue — a fila INTERNA do Claude Code registra o texto NO MOMENTO da digitacao, antes de
    virar entrada user. Sem (b), mensagem enfileirada durante um turno longo parecia 'engolida'
    e era REDIGITADA em loop (o bug das mensagens fantasma repetidas)."""
    out: set[str] = set()

    def add(t: str) -> None:
        # Indexa a variante CRUA e a SEM marcador de anexo: a msg do app e digitada COM o
        # "📎 imagem: <path>" na mesma linha (o transcript guarda a linha inteira), mas o reconcile
        # compara o texto podado — sem indexar as DUAS variantes, msg COM ANEXO nunca confirmava
        # e era redigitada (as duplicatas so-com-imagem de 2026-07-02).
        for variant in (t, _strip_attach(t)):
            variant = variant.strip()
            if not variant:
                continue
            out.add(variant)
            for ln in variant.split("\n"):
                out.add(ln.strip())

    try:
        with open(jsonl, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                try:
                    obj = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                etype = obj.get("type")
                if etype == "queue-operation":
                    c = obj.get("content")
                    if isinstance(c, str):
                        add(c)
                    continue
                if etype != "user":
                    continue
                content = (obj.get("message") or {}).get("content")
                if isinstance(content, str):
                    add(content)
                elif isinstance(content, list):
                    for b in content:
                        if isinstance(b, dict) and b.get("type") == "text":
                            add(str(b.get("text", "")))
    except OSError:
        pass
    return out


class PromptQueue:
    """Fila duravel de prompts por sessao (sidecar JSONL). Registra cada envio pra que msgs
    enfileiradas (mandadas com o Claude trabalhando) — que o Claude Code NEM sempre grava no
    proprio transcript — aparecam no fluxo, em ordem, e sobrevivam a reload. O merge dedup-a
    contra o transcript: quando o Claude Code grava o prompt real, a entrada da fila some."""

    def __init__(self, name: str):
        self.path = _queue_dir() / f"{_sanitize(name)}.jsonl"

    def _write_atomic(self, rows: list[dict]) -> None:
        # Escrita atomica (tmp + replace) pra um reader nunca pegar o arquivo pela metade.
        tmp = self.path.with_suffix(".jsonl.tmp")
        tmp.write_text(
            "".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows), encoding="utf-8"
        )
        tmp.replace(self.path)

    def append(self, text: str, delivered: bool = False) -> dict:
        # delivered=False por padrao = enfileirada mas NAO digitada na TUI (o /input passa True quando
        # o send_prompt realmente digitou). So entradas False sao drenadas -> sem isto um upgrade
        # re-enviaria toda entrada legada (= double-send em massa).
        entry = {"id": uuid.uuid4().hex, "text": text, "ts": time.time(), "delivered": delivered}
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

    def claim_undelivered(self, min_ts: float = 0.0, limit: int | None = None) -> list[dict]:
        """Reivindica (atomicamente) entradas ainda nao entregues: vira delivered=True e devolve as
        reivindicadas. Sob _append_lock -> com N drains concorrentes so UM pega cada entrada (os
        outros pegam []) = single-flight, sem double-send. `is False` ESTRITO: legada (sem a chave) ou
        ja entregue NAO entra. min_ts poda entradas de sessao antiga (pre-/clear)."""
        with _append_lock:
            rows = self.load()
            claimed = []
            for r in rows:
                if r.get("delivered") is False and float(r.get("ts") or 0.0) >= min_ts:
                    r["delivered"] = True
                    claimed.append(dict(r))
                    if limit is not None and len(claimed) >= limit:
                        break
            if claimed:
                self._write_atomic(rows)
            return claimed

    def set_delivered(self, entry_id: str, value: bool) -> None:
        """Marca UMA entrada (por id) como delivered=value e reescreve atomico. Usado pra reverter um
        claim quando o envio nao chegou a tocar a TUI (provadamente pre-envio)."""
        with _append_lock:
            rows = self.load()
            for r in rows:
                if str(r.get("id")) == entry_id:
                    r["delivered"] = value
                    break
            else:
                return
            self._write_atomic(rows)

    def prune_before(self, min_ts: float) -> None:
        # Entradas de sessao ANTERIOR (ts < inicio do transcript atual) nunca mais casam nem drenam
        # — so acumulavam lixo e mantinham o cheap-check do drain quente pra sempre. Remove.
        if min_ts <= 0:
            return
        with _append_lock:
            rows = self.load()
            kept = [r for r in rows if float(r.get("ts") or 0.0) >= min_ts]
            if len(kept) != len(rows):
                self._write_atomic(kept)

    def reconcile_delivered(self, committed: set[str], min_ts: float, now: float,
                            grace: float = 8.0, max_attempts: int = 2) -> list[dict]:
        """Confirma entregas contra o transcript ou RE-ENFILEIRA as engolidas. delivered=True quer
        dizer 'send_keys chamado', nao 'Claude recebeu' — a TUI pode engolir as teclas (redraw) e a
        msg sumia com cara de entregue. Entrada delivered, nao-confirmada, da sessao atual e mais
        velha que `grace`: texto no transcript -> confirmed=True (para de checar); ausente ->
        delivered=False + attempts+1 (o drain reentrega); attempts >= max_attempts -> desiste
        (confirmed=True: fica visivel como bubble = comportamento antigo, sem loop de redigitacao).
        Devolve as re-enfileiradas."""
        with _append_lock:
            rows = self.load()
            requeued: list[dict] = []
            changed = False
            for r in rows:
                if r.get("delivered") is not True or r.get("confirmed"):
                    continue
                ts = float(r.get("ts") or 0.0)
                if ts < min_ts:
                    r["confirmed"] = True   # sessao anterior: fora do escopo (e silencia o check)
                    changed = True
                    continue
                if now - ts < grace:
                    continue                # recente demais: o transcript pode nao ter gravado ainda
                # Compara CRU e podado (espelha o lado do committed_user_lines): so um dos lados
                # podado deixava msg com anexo orfa -> requeue indevido.
                text_raw = str(r.get("text") or "").strip()
                text = _strip_attach(text_raw).strip()
                lines = {text_raw, text,
                         *(ln.strip() for ln in text_raw.split("\n")),
                         *(ln.strip() for ln in text.split("\n"))}
                lines.discard("")
                if not text_raw or lines & committed:
                    r["confirmed"] = True
                elif int(r.get("attempts") or 0) >= max_attempts:
                    r["confirmed"] = True
                else:
                    r["delivered"] = False
                    r["attempts"] = int(r.get("attempts") or 0) + 1
                    requeued.append(dict(r))
                changed = True
            if changed:
                self._write_atomic(rows)
            return requeued

    def clear(self) -> None:
        # Remove o sidecar inteiro. Usado quando /clear reinicia a sessao do Claude Code: as entradas
        # pertencem ao transcript ANTIGO e nao devem reaparecer como bubble no transcript novo (a fila
        # e keyed pelo NOME da sessao, que sobrevive ao /clear -> sem isto, viram fantasma).
        self.path.unlink(missing_ok=True)
        self.path.with_suffix(".jsonl.tmp").unlink(missing_ok=True)

    def rename(self, new_name: str) -> None:
        # Move o sidecar pro nome novo, preservando entradas nao-drenadas (a fila e keyed pelo NOME;
        # sem mover, a sessao renomeada perderia a fila e ela viraria orfa no nome velho). Move atomico
        # (mesmo dir). Sem fila = no-op. O .tmp meio-escrito nao migra.
        self.path.with_suffix(".jsonl.tmp").unlink(missing_ok=True)
        if self.path.exists():
            self.path.replace(_queue_dir() / f"{_sanitize(new_name)}.jsonl")

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

    async def follow(self, min_ts: float = 0.0) -> AsyncIterator[ChatEvent]:
        # Emite as entradas existentes e depois vigia novos appends, como user_msg sintetico.
        # Usa um set de ids ja vistos (o append reescreve o arquivo inteiro -> rastrear posicao
        # quebraria; reload + dedup por id e simples e correto). min_ts: descarta entradas anteriores
        # ao inicio da sessao atual (ex: pre-/clear) — espelha a poda do merged_history no live SSE.
        seen: set[str] = set()

        def emit_new() -> list[ChatEvent]:
            evs = []
            for entry in self.load():
                eid = str(entry.get("id"))
                if not eid or eid in seen:
                    continue
                seen.add(eid)
                if min_ts and float(entry.get("ts") or 0.0) < min_ts:
                    continue
                # CONFIRMADA = texto comprovadamente no transcript (reconcile): a bolha real existe
                # -> re-emitir o eco so duplicava (bolha antiga "solta" no fim a cada reconexao).
                if entry.get("confirmed"):
                    continue
                evs.append(_entry_event(entry))
            return evs

        # emit_new() faz read_text do sidecar -> roda no threadpool pra nao bloquear o loop. As chamadas
        # sao sequenciais (uma await por vez), entao o set `seen` que ela muta nao corre risco de corrida.
        for ev in await asyncio.to_thread(emit_new):
            yield ev
        # yield_on_timeout: cobre entrada gravada entre o emit_new acima e o watcher armar (senao so
        # apareceria no proximo write da fila). O dir e COMPARTILHADO por todas as sessoes -> filtra:
        # so recarrega quando o toque e no NOSSO arquivo (ou no timeout do heartbeat).
        async for changes in awatch(self.path.parent, yield_on_timeout=True, rust_timeout=5000):
            if changes and not any(Path(p).name == self.path.name for _, p in changes):
                continue
            for ev in await asyncio.to_thread(emit_new):
                yield ev


def merged_history(name: str, jsonl: str, provider: str = "claude") -> list[ChatEvent]:
    """Historico = eventos do transcript + entradas da fila ainda NAO absorvidas pelo transcript,
    ordenado por timestamp. Dedup TS-AWARE: descarta entrada da fila cujo texto ja apareca (por
    linha) num user_msg commitado DEPOIS dela (o transcript grava o prompt apos a entrega). Match
    por texto sozinho engolia repeticao: o 2o "ok" enfileirado sumia por causa do 1o ja commitado.
    Entradas sem timestamp herdam o ts anterior (carry-forward) pra manter a ordem do arquivo.

    provider: qual parser usar pra `jsonl` -- Claude e Codex tem shapes diferentes (ver
    app.adapters.codex.rollout). _ts_of_obj fica igual pros dois: ambos gravam `timestamp` ISO
    no topo da linha. Import local do parser do Codex evita ciclo (app.adapters importa
    app.pqueue no boot, pra PromptQueue)."""
    if provider == "codex":
        from app.adapters.codex.rollout import parse_rollout_obj as _parse
    else:
        _parse = parse_obj
    items: list[tuple[float, int, ChatEvent]] = []
    committed_ts: dict[str, float] = {}  # linha normalizada -> maior ts em que commitou
    prev_ts = 0.0
    start_ts = 0.0  # 1o ts real do transcript = inicio da sessao atual (pra podar fila pre-/clear)
    try:
        fh = open(jsonl, encoding="utf-8", errors="replace")
    except OSError:
        fh = None  # sessao nova: jsonl ainda nao existe -> historico vazio (limpo), nao 500
    if fh is not None:
        # Itera linha-a-linha (nao carrega o transcript inteiro em RAM) e parseia o JSON UMA vez por
        # linha (antes parse_line + _ts_of_line = 2x json.loads do arquivo inteiro por /history).
        with fh:
            for i, line in enumerate(fh):
                try:
                    obj = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                evs = _parse(obj)
                if not evs:
                    continue
                line_ts = _ts_of_obj(obj)
                if line_ts > 0 and start_ts == 0.0:
                    start_ts = line_ts
                ts = line_ts or prev_ts
                prev_ts = ts
                for ev in evs:
                    items.append((ts, i, ev))
                    if ev.kind == "user_msg" and ev.text:
                        t = ev.text.strip()
                        for ln in (t, *(s.strip() for s in t.split("\n"))):
                            if ts > committed_ts.get(ln, 0.0):
                                committed_ts[ln] = ts

    # Entradas da fila entram com tiebreaker alto -> caem DEPOIS de eventos do transcript de mesmo ts.
    for entry in PromptQueue(name).load():
        if entry.get("confirmed"):
            continue  # comprovadamente no transcript (reconcile) -> a bolha real ja cobre
        text = (entry.get("text") or "").strip()
        if not text:
            continue
        ts = float(entry.get("ts") or prev_ts)
        # Absorvida so se o texto commitou DEPOIS de enfileirada (>=: o write real e sempre
        # posterior ao append da fila). Commit ANTERIOR e de outra msg igual -> esta segue pendente.
        if committed_ts.get(text, -1.0) >= ts:
            continue
        # Poda: entrada anterior ao inicio da sessao atual e de uma sessao antiga (ex: pre-/clear, que
        # cria transcript novo). Sem isto, nunca casaria com o transcript novo e viraria fantasma.
        if start_ts and ts < start_ts:
            continue
        items.append((ts, 10**9, _entry_event(entry)))

    items.sort(key=lambda x: (x[0], x[1]))
    return [ev for _, _, ev in items]
