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
from app.transcript import parse_line

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


def _ts_of_line(line: str) -> float:
    # Epoch (s) do campo `timestamp` (ISO 8601 com Z) de uma linha do transcript; 0.0 se ausente.
    try:
        obj = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return 0.0
    t = obj.get("timestamp")
    if not isinstance(t, str):
        return 0.0
    try:
        return datetime.fromisoformat(t.replace("Z", "+00:00")).timestamp()
    except ValueError:
        return 0.0


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
                evs.append(_entry_event(entry))
            return evs

        # emit_new() faz read_text do sidecar -> roda no threadpool pra nao bloquear o loop. As chamadas
        # sao sequenciais (uma await por vez), entao o set `seen` que ela muta nao corre risco de corrida.
        for ev in await asyncio.to_thread(emit_new):
            yield ev
        async for _ in awatch(self.path.parent):
            for ev in await asyncio.to_thread(emit_new):
                yield ev


def merged_history(name: str, jsonl: str) -> list[ChatEvent]:
    """Historico = eventos do transcript + entradas da fila ainda NAO absorvidas pelo transcript,
    ordenado por timestamp. Dedup: descarta entrada da fila cujo texto ja apareca (por linha) num
    user_msg do transcript (o Claude Code consumiu o prompt -> a versao real vence). Entradas sem
    timestamp herdam o ts anterior (carry-forward) pra manter a ordem do arquivo."""
    items: list[tuple[float, int, ChatEvent]] = []
    committed_lines: set[str] = set()
    prev_ts = 0.0
    start_ts = 0.0  # 1o ts real do transcript = inicio da sessao atual (pra podar fila pre-/clear)
    try:
        raw = Path(jsonl).read_text(encoding="utf-8", errors="replace")
    except OSError:
        raw = ""  # sessao nova: jsonl ainda nao existe -> historico vazio (limpo), nao 500
    for i, line in enumerate(raw.splitlines()):
        ev = parse_line(line)
        if ev is None:
            continue
        line_ts = _ts_of_line(line)
        if line_ts > 0 and start_ts == 0.0:
            start_ts = line_ts
        ts = line_ts or prev_ts
        prev_ts = ts
        items.append((ts, i, ev))
        if ev.kind == "user_msg" and ev.text:
            t = ev.text.strip()
            committed_lines.add(t)
            for ln in t.split("\n"):
                committed_lines.add(ln.strip())

    # Entradas da fila entram com tiebreaker alto -> caem DEPOIS de eventos do transcript de mesmo ts.
    for entry in PromptQueue(name).load():
        text = (entry.get("text") or "").strip()
        if not text or text in committed_lines:
            continue
        ts = float(entry.get("ts") or prev_ts)
        # Poda: entrada anterior ao inicio da sessao atual e de uma sessao antiga (ex: pre-/clear, que
        # cria transcript novo). Sem isto, nunca casaria com o transcript novo e viraria fantasma.
        if start_ts and ts < start_ts:
            continue
        items.append((ts, 10**9, _entry_event(entry)))

    items.sort(key=lambda x: (x[0], x[1]))
    return [ev for _, _, ev in items]
