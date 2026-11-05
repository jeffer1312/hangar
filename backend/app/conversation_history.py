"""História congelada Claude e apenas os turnos novos da thread Codex."""
from __future__ import annotations

import base64
import hashlib
import json
import os
import threading
from dataclasses import asdict
from pathlib import Path

from app.conversation_transfer import (
    ConversationSource, TransferPhase, TransferRecord, transfer_for_session, transfer_for_thread,
)
from app.models import ChatEvent, SessionInfo


class HistoryError(ValueError):
    """Uma fonte inválida nunca autoriza confirmar a fila ou responder conversa vazia."""


_verification_lock = threading.Lock()
_verified_prefixes: dict[tuple[str, int, str | None], tuple[tuple, str, dict]] = {}
_verified_sources: dict[tuple[str, str], tuple] = {}
_verified_active: dict[tuple, tuple[tuple, str]] = {}
_VERIFICATION_CACHE_SIZE = 32


def _signature(stat) -> tuple:
    return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns


def _remember(cache: dict, key, value) -> None:
    cache.pop(key, None)
    cache[key] = value
    while len(cache) > _VERIFICATION_CACHE_SIZE:
        del cache[next(iter(cache))]


def verified_prefix(path: str | Path, min_offset: int, expected_digest: str | None) -> tuple[str, dict]:
    """Uma leitura por revisão do arquivo; append, rewrite e truncamento invalidam a prova."""
    signature = _signature(Path(path).stat())
    key = (os.path.realpath(path), min_offset, expected_digest)
    with _verification_lock:
        cached = _verified_prefixes.get(key)
    if cached and cached[0] == signature:
        return cached[1], cached[2]
    digest = hashlib.sha256()
    with Path(path).open("rb") as fh:
        header_raw = fh.readline(min_offset)
        digest.update(header_raw)
        remaining = min_offset - len(header_raw)
        last = header_raw[-1:]
        while remaining:
            chunk = fh.read(min(remaining, 1024 * 1024))
            if not chunk:
                raise HistoryError("o rollout foi truncado antes da importação")
            digest.update(chunk)
            remaining -= len(chunk)
            last = chunk[-1:]
    if last != b"\n":
        raise HistoryError("a fronteira do rollout tem uma linha incompleta")
    actual = digest.hexdigest()
    if expected_digest and actual != expected_digest:
        raise HistoryError("o prefixo importado do rollout mudou")
    if _signature(Path(path).stat()) != signature:
        raise HistoryError("o rollout mudou durante a verificação da fronteira")
    try:
        header = json.loads(header_raw)
    except ValueError as exc:
        raise HistoryError("o rollout perdeu a identidade da thread") from exc
    if not isinstance(header, dict):
        raise HistoryError("o rollout perdeu a identidade da thread")
    with _verification_lock:
        _remember(_verified_prefixes, key, (signature, actual, header))
    return actual, header


def verify_source_snapshot(source: ConversationSource) -> None:
    key = (os.path.realpath(source.path), source.digest)
    try:
        signature = _signature(Path(source.path).stat())
        with _verification_lock:
            verified = _verified_sources.get(key)
        if verified == signature:
            return
        digest = hashlib.sha256()
        with Path(source.path).open("rb") as fh:
            while chunk := fh.read(1024 * 1024):
                digest.update(chunk)
        if digest.hexdigest() != source.digest or _signature(Path(source.path).stat()) != signature:
            raise HistoryError("o snapshot da origem mudou")
        with _verification_lock:
            _remember(_verified_sources, key, signature)
    except OSError as exc:
        raise HistoryError("o snapshot da origem está indisponível") from exc


def session_transfer(name: str, jsonl: str, provider: str) -> TransferRecord | None:
    if provider != "codex":
        return None
    try:
        record = transfer_for_session(name, source_path=jsonl)
    except (ValueError, OSError) as exc:
        raise HistoryError("o registro da transferência está indisponível") from exc
    if not record:
        record = archive_transfer(jsonl)
    if not record:
        return None
    if record.phase != TransferPhase.COMPLETE:
        raise HistoryError("a transferência ainda não foi concluída")
    boundary = record.boundary
    if not record.source or not boundary:
        raise HistoryError("o registro concluído perdeu uma fonte da conversa")
    if not _matches_rollout(record, jsonl):
        # /clear mantém a vida lógica, mas inicia outra conversa.
        return None
    return record


def archive_transfer(path: str | Path) -> TransferRecord | None:
    from app.codex_contas import account_for_rollout
    from app.models import session_key
    owner = account_for_rollout(Path(path))
    if owner is None:
        return None
    try:
        record = transfer_for_thread(str(owner.home), session_key(str(path)))
    except (ValueError, OSError) as exc:
        raise HistoryError("o registro histórico da thread está indisponível") from exc
    if record and record.phase != TransferPhase.COMPLETE:
        raise HistoryError("a preparação da thread não é uma conversa concluída")
    return record


def _matches_rollout(record: TransferRecord, path: str | Path) -> bool:
    boundary = record.boundary
    if not boundary:
        return False
    if os.path.realpath(path) == os.path.realpath(boundary.rollout_path):
        return True
    from app.codex_contas import account_for_rollout
    from app.models import session_key
    # O Codex move uma conversa encerrada para archived_sessions na mesma conta.
    owner = account_for_rollout(Path(path))
    home = (record.destination_meta or {}).get("codex_home")
    return bool(owner and home and os.path.realpath(owner.home) == os.path.realpath(home)
                and session_key(str(path)) == boundary.thread_id)


def conversation_sources(info: SessionInfo) -> tuple[ConversationSource, ...]:
    if not info.jsonl:
        return ()
    active = ConversationSource(info.jsonl, info.provider, "", ())
    record = session_transfer(info.name, info.jsonl, info.provider)
    return (record.source, active) if record and record.source else (active,)


def source_rows(source: ConversationSource) -> list[dict]:
    from app.claude_to_codex import reconstruct_chain
    try:
        raw = Path(source.path).read_bytes()
    except OSError as exc:
        raise HistoryError("o snapshot da origem está indisponível") from exc
    if hashlib.sha256(raw).hexdigest() != source.digest:
        raise HistoryError("o snapshot da origem mudou")
    if not raw.endswith(b"\n"):
        raise HistoryError("o snapshot da origem tem linha incompleta")
    try:
        rows = [json.loads(line) for line in raw.decode("utf-8").split("\n")[:-1]]
        chain = reconstruct_chain(rows)
    except (ValueError, UnicodeError) as exc:
        raise HistoryError("o ramo da origem não pode ser reconstruído") from exc
    if {row.get("uuid") for row in chain} != set(source.selected_uuids):
        raise HistoryError("o ramo selecionado diverge do snapshot")
    return chain


def verify_boundary(record: TransferRecord, path: str | Path) -> int:
    boundary = record.boundary
    if record.phase != TransferPhase.COMPLETE or not record.source or not boundary:
        raise HistoryError("a transferência não possui história concluída")
    if not _matches_rollout(record, path):
        raise HistoryError("o rollout não pertence à transferência")
    digest = getattr(boundary, "prefix_digest", None)
    if not digest:
        raise HistoryError("a fronteira da importação não possui digest verificado")
    try:
        _, header = verified_prefix(path, boundary.min_offset, digest)
    except OSError as exc:
        raise HistoryError("o rollout da transferência está indisponível") from exc
    payload = header.get("payload") if isinstance(header, dict) else None
    if not isinstance(payload, dict):
        raise HistoryError("o rollout perdeu a identidade da thread")
    if header.get("type") != "session_meta" or payload.get("id", payload.get("session_id")) != boundary.thread_id:
        raise HistoryError("a thread do rollout diverge da transferência")
    return boundary.min_offset


def active_rows(record: TransferRecord, path: str | Path):
    offset = verify_boundary(record, path)
    with Path(path).open("rb") as fh:
        fh.seek(offset)
        while raw := fh.readline():
            start = offset
            offset = fh.tell()
            if not raw.endswith(b"\n"):
                break  # A próxima leitura só publica a linha completa.
            try:
                row = json.loads(raw.decode("utf-8"))
            except (ValueError, UnicodeError) as exc:
                raise HistoryError("o rollout ativo contém uma linha inválida") from exc
            if not isinstance(row, dict):
                raise HistoryError("o rollout ativo contém um registro inválido")
            payload = row.get("payload") or {}
            item_id = payload.get("id") if isinstance(payload, dict) else None
            if item_id in record.boundary.imported_item_ids or row.get("id") in record.boundary.imported_item_ids:
                continue
            yield start, row


def source_event(record: TransferRecord, event: ChatEvent, provider: str) -> ChatEvent:
    prefix = f"transfer:{record.id}:{provider}:"
    return event.model_copy(update={
        "id": prefix + event.id,
        "tool_use_id": prefix + event.tool_use_id if event.tool_use_id else None,
    })


def composed_history(name: str, jsonl: str, provider: str, record: TransferRecord,
                     limit: int | None, *, include_queue: bool = True) -> list[ChatEvent]:
    from app.adapters.codex.rollout import parse_rollout_obj
    from app.pqueue import _chaves_de_commit, _ts_of_obj, merge_current_queue
    from app.transcript import parse_obj
    if provider != "codex" or record.source is None or record.source.provider != "claude":
        raise HistoryError("a composição exige origem Claude e destino Codex")
    rows = source_rows(record.source)
    old: list[ChatEvent] = []
    previous = 0.0
    start_ts = 0.0
    for row in rows:
        previous = _ts_of_obj(row) or previous
        start_ts = start_ts or previous
        for event in parse_obj(row):
            old.append(source_event(record, event.model_copy(update={"ts": event.ts or previous}), "claude"))
    active: list[tuple[float, int, ChatEvent]] = []
    committed_ts: dict[str, float] = {}
    for offset, row in active_rows(record, jsonl):
        previous = _ts_of_obj(row) or previous
        for event in parse_rollout_obj(row):
            ts = event.ts or previous
            event = source_event(record, event.model_copy(update={"ts": ts}), "codex")
            active.append((ts, offset, event))
            if event.kind == "user_msg" and event.text:
                for text in _chaves_de_commit(event.text):
                    committed_ts[text] = max(ts, committed_ts.get(text, 0.0))
    # A origem mantém a ordem do ramo. Somente os commits novos reconciliam a fila atual.
    new = (merge_current_queue(name, active, committed_ts, previous, start_ts)
           if include_queue else [event for _, _, event in active])
    events = old + new
    return events[-limit:] if limit is not None and limit > 0 else events


def composition_etag(name: str, jsonl: str, provider: str, record: TransferRecord,
                     limit: int | None, code_marker: int) -> str:
    from app.pqueue import PromptQueue
    verify_source_snapshot(record.source)
    verify_boundary(record, jsonl)
    stat = Path(jsonl).stat()
    active_key = (os.path.realpath(jsonl), record.boundary.min_offset,
                  record.boundary.prefix_digest, record.boundary.imported_item_ids)
    with _verification_lock:
        cached = _verified_active.get(active_key)
    if cached and cached[0] == _signature(stat):
        active_digest = cached[1]
    else:
        digest = hashlib.sha256()
        for _, row in active_rows(record, jsonl):
            digest.update(json.dumps(row, sort_keys=True).encode())
        if _signature(Path(jsonl).stat()) != _signature(stat):
            raise HistoryError("o rollout mudou durante a leitura do histórico")
        active_digest = digest.hexdigest()
        with _verification_lock:
            _remember(_verified_active, active_key, (_signature(stat), active_digest))
    queue = PromptQueue(name).path
    try:
        queue_stat = queue.stat()
        queue_marker = _signature(queue_stat)
    except FileNotFoundError:
        queue_marker = None
    marker = (asdict(record), active_digest, _signature(stat),
              queue_marker, provider, limit, code_marker)
    digest = hashlib.sha256(json.dumps(marker, sort_keys=True, default=str).encode()).hexdigest()
    return f'"{digest}"'


def confirmation_boundary(name: str, path: str, provider: str) -> tuple[int, str | None]:
    options = confirmation_options(name, path, provider)
    return options.get("min_offset", 0), options.get("prefix_digest")


def confirmation_options(name: str, path: str, provider: str) -> dict:
    record = session_transfer(name, path, provider)
    if not record:
        return {}
    verify_source_snapshot(record.source)
    offset = verify_boundary(record, path)
    return {"min_offset": offset, "prefix_digest": record.boundary.prefix_digest,
            "imported_item_ids": record.boundary.imported_item_ids}


def live_event(record: TransferRecord, path: str, event: ChatEvent) -> ChatEvent | None:
    offset = verify_boundary(record, path)
    if event.offset is None:
        raise HistoryError("evento ativo sem fronteira de leitura")
    if event.offset < offset:
        return None
    with Path(path).open("rb") as fh:
        fh.seek(event.offset)
        raw = fh.readline()
    if not raw.endswith(b"\n"):
        return None
    row = json.loads(raw)
    payload = row.get("payload") or {}
    if payload.get("id") in record.boundary.imported_item_ids or row.get("id") in record.boundary.imported_item_ids:
        return None
    return source_event(record, event, "codex")


def citation_rows(info):
    provider = getattr(info, "provider", "claude")
    if provider != "codex":
        return None
    record = session_transfer(info.name, info.jsonl, provider)
    if not record:
        return None
    old = source_rows(record.source)
    new = [row for _, row in active_rows(record, info.jsonl)]
    # Os mesmos bytes que os leitores atuais inspecionariam, limitados às fontes autorizadas.
    return [json.dumps(row, ensure_ascii=False).encode() + b"\n" for row in old + new]


def historical_image(record: TransferRecord, event_id: str, index: int):
    prefix = f"transfer:{record.id}:claude:"
    if not event_id.startswith(prefix):
        return None
    uuid = event_id.removeprefix(prefix)
    for row in source_rows(record.source):
        if row.get("uuid") != uuid:
            continue
        content = (row.get("message") or {}).get("content")
        images = [block for block in content if isinstance(block, dict) and block.get("type") == "image"] if isinstance(content, list) else []
        if not 0 <= index < len(images):
            return None
        source = images[index].get("source") or {}
        if source.get("type") != "base64" or not isinstance(source.get("data"), str):
            return None
        try:
            return base64.b64decode(source["data"], validate=True), source.get("media_type", "image/png")
        except (ValueError, base64.binascii.Error):
            return None
    return None


def transcript_image(info, event_id: str, index: int):
    from app.transcript import get_transcript_image
    provider = getattr(info, "provider", "claude")
    if provider != "codex":
        return get_transcript_image(info.jsonl, event_id, index)
    record = session_transfer(info.name, info.jsonl, provider)
    if not record:
        return get_transcript_image(info.jsonl, event_id, index)
    return historical_image(record, event_id, index)
