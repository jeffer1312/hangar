"""Regressões da fonte congelada, fronteira de importação e recursos históricos."""
import base64
from dataclasses import replace
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from app import conversation_history as history, conversation_transfer as store, pqueue
from app.conversation_transfer import ConversationSource, ImportBoundary, TransferPhase, TransferRecord
from app.models import SessionInfo

THREAD = "019f99de-9572-7221-89c0-80c62f883d44"
TRANSFER = "a563a017-af37-42e8-a8e5-2bd837584f8e"
REAL_ARCHIVE_TRANSFER = history.archive_transfer


def encoded(row):
    return (json.dumps(row, ensure_ascii=False) + "\n").encode()


def codex_message(text, *, item_id="new", role="user", day=2):
    return {"timestamp": f"2026-02-{day:02d}T00:00:00Z", "type": "response_item",
            "payload": {"type": "message", "id": item_id, "role": role,
                        "content": [{"type": "input_text" if role == "user" else "output_text", "text": text}]}}


@pytest.fixture
def joined(tmp_path, monkeypatch):
    queue_dir = tmp_path / "queue"
    queue_dir.mkdir()
    monkeypatch.setattr(pqueue, "_queue_dir", lambda: queue_dir)
    monkeypatch.setattr(pqueue, "_indices", {})
    new = codex_message("novo")
    from app.adapters.codex.rollout import _event_id
    collision = _event_id(new)
    rows = [
        {"type": "user", "uuid": collision, "parentUuid": None,
         "timestamp": "2026-01-01T00:00:00Z", "cwd": str(tmp_path),
         "message": {"role": "user", "content": "ok"}},
        {"type": "assistant", "uuid": "tool", "parentUuid": collision,
         "timestamp": "2026-01-01T00:01:00Z", "message": {"role": "assistant", "content": [
             {"type": "tool_use", "id": "call", "name": "Read", "input": {"file_path": "/original/report.md"}}]}},
        {"type": "user", "uuid": "result", "parentUuid": "tool",
         "timestamp": "2026-01-01T00:02:00Z", "message": {"role": "user", "content": [
             {"type": "tool_result", "tool_use_id": "call", "content": "resultado original"}]}},
        {"type": "system", "subtype": "compact_boundary", "uuid": "compact", "parentUuid": None,
         "logicalParentUuid": "result"},
        {"type": "user", "uuid": "summary", "parentUuid": "compact", "isCompactSummary": True,
         "message": {"role": "user", "content": "RESUMO NÃO DEVE ENTRAR"}},
        {"type": "assistant", "uuid": "after", "parentUuid": "summary",
         "timestamp": "2026-01-01T00:03:00Z", "message": {"role": "assistant", "content": "antigo"}},
        {"type": "assistant", "uuid": "fork", "parentUuid": collision, "isSidechain": True,
         "message": {"role": "assistant", "content": "FORK NÃO DEVE ENTRAR"}},
        {"type": "last-prompt", "leafUuid": "after"},
    ]
    original = tmp_path / "original.jsonl"
    original.write_bytes(b"".join(map(encoded, rows)))
    snapshot = tmp_path / "snapshot.jsonl"
    snapshot.write_bytes(original.read_bytes())
    selected = tuple(row["uuid"] for row in rows[:6] if not row.get("isCompactSummary"))
    source = ConversationSource(str(snapshot), "claude", hashlib.sha256(snapshot.read_bytes()).hexdigest(), selected)
    prefix = encoded({"type": "session_meta", "payload": {"id": THREAD, "cwd": str(tmp_path)}})
    prefix += encoded(codex_message("ok", item_id="imported", day=1))
    path = tmp_path / f"rollout-fixture-{THREAD}.jsonl"
    path.write_bytes(prefix + encoded(new))
    boundary = ImportBoundary(THREAD, str(path), len(prefix), ("imported",),
                              prefix_digest=hashlib.sha256(prefix).hexdigest())
    record = TransferRecord(TRANSFER, "s", "k:original", TransferPhase.COMPLETE, source,
                            {"provider": "claude", "jsonl": str(original), "cwd": str(tmp_path), "key": "original"},
                            {"provider": "codex", "thread_id": THREAD, "codex_home": str(tmp_path / "codex"),
                             "lifecycle_id": "k:original", "transfer_id": TRANSFER}, boundary, None)
    monkeypatch.setattr(history, "transfer_for_session", lambda *args, **kwargs: record)
    monkeypatch.setattr(history, "archive_transfer", lambda path: None)
    info = SessionInfo(name="s", jsonl=str(path), provider="codex", cwd=str(tmp_path))
    return SimpleNamespace(record=record, path=path, original=original, snapshot=snapshot, rows=rows,
                           info=info, prefix=prefix, collision=collision)


def compose(joined, limit=None):
    return history.composed_history("s", str(joined.path), "codex", joined.record, limit)


def test_join_excludes_import_fork_summary_and_preserves_tool_pairs(joined):
    events = compose(joined)
    assert [event.text or event.result for event in events] == ["ok", None, "resultado original", "antigo", "novo"]
    assert events[0].id.endswith(":claude:" + joined.collision)
    assert events[-1].id.endswith(":codex:" + joined.collision)
    assert len({event.id for event in events}) == len(events)
    assert events[1].tool_use_id == events[2].tool_use_id
    assert compose(joined, 3) == events[-3:]
    with joined.original.open("ab") as fh:
        fh.write(encoded({"type": "user", "uuid": "later", "message": {"content": "FORA DO SNAPSHOT"}}))
    assert compose(joined) == events


def test_origin_and_imported_ok_never_absorb_current_queue(joined):
    entry = pqueue.PromptQueue("s").append("ok", delivered=True, ts=1769990400)
    events = compose(joined)
    assert events[-1].id == "queued-" + entry["id"]
    assert sum(event.id.startswith("queued-") for event in events) == 1
    assert "ok" not in pqueue.committed_user_lines(str(joined.path), "codex", min_offset=joined.record.boundary.min_offset,
                                                   prefix_digest=joined.record.boundary.prefix_digest)
    with joined.path.open("ab") as fh:
        fh.write(encoded(codex_message("ok", day=4)))
    assert "ok" in pqueue.committed_user_lines(str(joined.path), "codex", min_offset=joined.record.boundary.min_offset,
                                               prefix_digest=joined.record.boundary.prefix_digest)
    assert not any(event.id.startswith("queued-") for event in compose(joined))


def test_partial_active_line_does_not_confirm_queue(joined):
    line = encoded(codex_message("ok", day=4))
    with joined.path.open("ab") as fh:
        fh.write(line[:-1])
    assert [event.text for event in compose(joined) if event.kind == "user_msg"] == ["ok", "novo"]
    assert "ok" not in pqueue.committed_user_lines(str(joined.path), "codex", min_offset=joined.record.boundary.min_offset,
                                                   prefix_digest=joined.record.boundary.prefix_digest)
    with joined.path.open("ab") as fh:
        fh.write(b"\n")
    assert "ok" in pqueue.committed_user_lines(str(joined.path), "codex", min_offset=joined.record.boundary.min_offset,
                                               prefix_digest=joined.record.boundary.prefix_digest)


@pytest.mark.parametrize("change", ["rewrite", "truncate", "missing_digest"])
def test_invalid_boundary_is_never_reinterpreted_as_fresh_commits(joined, change):
    boundary = joined.record.boundary
    if change == "rewrite":
        joined.path.write_bytes(joined.path.read_bytes().replace(b'"text": "ok"', b'"text": "no"'))
    elif change == "truncate":
        joined.path.write_bytes(joined.prefix[:boundary.min_offset - 1])
    else:
        joined.record = replace(joined.record, boundary=replace(boundary, prefix_digest=None))
    with pytest.raises(history.HistoryError):
        compose(joined)
    if change != "missing_digest":
        assert pqueue.committed_user_lines(str(joined.path), "codex", min_offset=boundary.min_offset,
                                           prefix_digest=boundary.prefix_digest) is None


def test_etag_tracks_verified_source_boundary_provider_limit_queue_and_code(joined):
    def etag(record=joined.record, limit=None, code=1):
        return history.composition_etag("s", str(joined.path), "codex", record, limit, code)
    initial = etag()
    assert etag(limit=3) != initial
    assert etag(code=2) != initial
    pqueue.PromptQueue("s").append("pendente")
    assert etag() != initial
    joined.snapshot.write_bytes(joined.snapshot.read_bytes().replace(b'"antigo"', b'"outro"'))
    changed = replace(joined.record, source=replace(joined.record.source,
                      digest=hashlib.sha256(joined.snapshot.read_bytes()).hexdigest()))
    assert etag(changed) != initial
    with pytest.raises(history.HistoryError):
        etag()


def test_lifecycle_new_thread_does_not_inherit_source_but_renamed_thread_does(joined, monkeypatch):
    assert history.session_transfer("renamed", str(joined.path), "codex") == joined.record
    clear = joined.path.with_name("new-thread.jsonl")
    clear.write_bytes(encoded({"type": "session_meta", "payload": {"id": "different"}}))
    monkeypatch.setattr(history, "_matches_rollout", lambda record, path: path == str(joined.path))
    assert history.session_transfer("s", str(clear), "codex") is None
    assert history.session_transfer("s", str(joined.path), "claude") is None


def test_archive_uses_account_thread_after_name_sidecar_closed(joined, monkeypatch, tmp_path):
    from app import codex_contas
    monkeypatch.setattr(store, "_base", lambda: tmp_path / "transfers")
    store.save_transfer(joined.record)
    monkeypatch.setattr(history, "transfer_for_session", lambda *args, **kwargs: None)
    monkeypatch.setattr(codex_contas, "account_for_rollout", lambda path: SimpleNamespace(
        home=Path(joined.record.destination_meta["codex_home"])))
    monkeypatch.setattr(history, "archive_transfer", REAL_ARCHIVE_TRANSFER)
    assert history.session_transfer("resumed-name", str(joined.path), "codex") == joined.record


def test_live_frames_below_boundary_and_import_replays_are_filtered(joined):
    from app.adapters.codex.rollout import parse_rollout_obj
    imported = parse_rollout_obj(codex_message("ok", item_id="imported", day=1))[0]
    imported.offset = len(joined.prefix.splitlines(keepends=True)[0])
    assert history.live_event(joined.record, str(joined.path), imported) is None
    row = codex_message("novo")
    active = parse_rollout_obj(row)[0]
    active.offset = joined.record.boundary.min_offset
    assert history.live_event(joined.record, str(joined.path), active).id == compose(joined)[-1].id
    with joined.path.open("ab") as fh:
        offset = fh.tell()
        fh.write(encoded(codex_message("ok", item_id="imported", day=5)))
    replay = imported.model_copy(update={"offset": offset})
    assert history.live_event(joined.record, str(joined.path), replay) is None
    assert [event.text for event in compose(joined) if event.kind == "user_msg"] == ["ok", "novo"]


def client_for(joined, monkeypatch):
    from app import api
    from app.config import settings
    monkeypatch.setattr(settings, "auth_token", "history-test-token")
    monkeypatch.setattr(api, "_cached_info_sync", lambda name: joined.info)
    async def info(name):
        return joined.info
    monkeypatch.setattr(api, "_cached_info", info)
    return TestClient(api.app), {"Authorization": "Bearer history-test-token"}


def test_history_api_rejects_source_failure_instead_of_304(joined, monkeypatch):
    client, headers = client_for(joined, monkeypatch)
    first = client.get("/api/sessions/s/history", headers=headers)
    assert first.status_code == 200
    assert len(first.json()) == 5
    joined.snapshot.unlink()
    failed = client.get("/api/sessions/s/history", headers={**headers, "If-None-Match": first.headers["etag"]})
    assert failed.status_code == 409


def test_historical_image_and_file_get_write_keep_authorization_and_digest(joined, monkeypatch):
    target = joined.snapshot.parent / "report.md"
    target.write_text("original", encoding="utf-8")
    image = b"original-image-bytes"
    row = joined.rows[0]
    row["message"]["content"] = [{"type": "text", "text": str(target)}, {"type": "image", "source": {
        "type": "base64", "media_type": "image/png", "data": base64.b64encode(image).decode()}}]
    joined.snapshot.write_bytes(b"".join(map(encoded, joined.rows)))
    joined.record = replace(joined.record, source=replace(joined.record.source,
                            digest=hashlib.sha256(joined.snapshot.read_bytes()).hexdigest()))
    monkeypatch.setattr(history, "transfer_for_session", lambda *args, **kwargs: joined.record)
    client, headers = client_for(joined, monkeypatch)
    event_id = compose(joined)[0].id
    picture = client.get(f"/api/sessions/s/transcript-image/{event_id}/0", headers=headers)
    assert picture.status_code == 200
    assert picture.content == image and picture.headers["content-type"] == "image/png"
    joined.snapshot.unlink()
    assert client.get(f"/api/sessions/s/transcript-image/{event_id}/0", headers=headers).status_code == 409


def test_replayed_import_after_boundary_cannot_confirm_new_ok(joined):
    with joined.path.open("ab") as fh:
        fh.write(encoded(codex_message("ok", item_id="imported", day=5)))
    assert "ok" not in pqueue.committed_user_lines(str(joined.path), "codex",
                                                   **history.confirmation_options("s", str(joined.path), "codex"))


def test_invalid_active_json_preserves_confirmation_queue(joined):
    with joined.path.open("ab") as fh:
        fh.write(b"invalid-json\n")
    assert pqueue.committed_user_lines(str(joined.path), "codex",
                                       **history.confirmation_options("s", str(joined.path), "codex")) is None
    with pytest.raises(history.HistoryError):
        history.composition_etag("s", str(joined.path), "codex", joined.record, None, 1)


def test_prefix_verification_cache_is_invalidated_even_when_mtime_is_restored(joined, monkeypatch):
    import os
    monkeypatch.setattr(history, "_verified_prefixes", {})
    original_open = Path.open
    reads = []
    def open_path(path, *args, **kwargs):
        if path == joined.path:
            reads.append(path)
        return original_open(path, *args, **kwargs)
    monkeypatch.setattr(Path, "open", open_path)
    history.verify_boundary(joined.record, joined.path)
    history.verify_boundary(joined.record, joined.path)
    assert len(reads) == 1
    old = joined.path.stat()
    joined.path.write_bytes(joined.path.read_bytes().replace(b'"text": "ok"', b'"text": "no"'))
    os.utime(joined.path, ns=(old.st_atime_ns, old.st_mtime_ns))
    with pytest.raises(history.HistoryError):
        history.verify_boundary(joined.record, joined.path)


def test_complete_record_cannot_silently_lose_one_history_source(joined, monkeypatch):
    sources = history.conversation_sources(joined.info)
    assert sources[0] == joined.record.source
    assert sources[1].path == str(joined.path) and sources[1].provider == "codex"
    broken = replace(joined.record, boundary=None)
    monkeypatch.setattr(history, "transfer_for_session", lambda *args, **kwargs: broken)
    with pytest.raises(history.HistoryError):
        history.session_transfer("s", str(joined.path), "codex")


def test_ack_suffix_rewrite_before_anchor_with_restored_mtime_cannot_confirm_new_ok(joined):
    import os
    suffix = encoded(codex_message("ok", item_id="real-ok", day=4))
    suffix += encoded(codex_message("padding" * 100, item_id="tail", role="assistant", day=4))
    joined.path.write_bytes(joined.prefix + suffix)
    options = history.confirmation_options("s", str(joined.path), "codex")
    assert "ok" in pqueue.committed_user_lines(str(joined.path), "codex", **options)
    before = joined.path.stat()
    tail_anchor = joined.path.read_bytes()[-256:]
    rewritten = suffix.replace(b'"text": "ok"', b'"text": "no"', 1)
    joined.path.write_bytes(joined.prefix + rewritten)
    os.utime(joined.path, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert joined.path.read_bytes()[:len(joined.prefix)] == joined.prefix
    assert joined.path.read_bytes()[-256:] == tail_anchor
    assert joined.path.stat().st_size == before.st_size
    assert joined.path.stat().st_mtime_ns == before.st_mtime_ns
    assert "ok" not in pqueue.committed_user_lines(str(joined.path), "codex", **options)


def test_ack_recheck_rejects_suffix_rewrite_during_read_even_with_restored_mtime(joined, monkeypatch):
    import os
    from app.adapters.codex.rollout import parse_rollout_obj
    suffix = encoded(codex_message("ok", item_id="real-ok", day=4))
    suffix += encoded(codex_message("padding" * 100, item_id="tail", role="assistant", day=4))
    joined.path.write_bytes(joined.prefix + suffix)
    before = joined.path.stat()
    options = history.confirmation_options("s", str(joined.path), "codex")
    rewritten = False
    def parse(row):
        nonlocal rewritten
        if not rewritten and (row.get("payload") or {}).get("id") == "real-ok":
            rewritten = True
            changed = suffix.replace(b'"text": "ok"', b'"text": "no"', 1)
            joined.path.write_bytes(joined.prefix + changed)
            os.utime(joined.path, ns=(before.st_atime_ns, before.st_mtime_ns))
        return parse_rollout_obj(row)
    monkeypatch.setattr(pqueue._CommittedIndex, "_parser", lambda self: parse)
    assert pqueue.committed_user_lines(str(joined.path), "codex", **options) is None
    assert rewritten


@pytest.mark.parametrize("newline", [b"\n", b"\r\n"])
def test_literal_unicode_separators_survive_history_citations_and_archive(joined, monkeypatch, tmp_path, newline):
    from app.claude_to_codex import convert_snapshot
    target = tmp_path / "literal-report.md"
    target.write_text("arquivo citado", encoding="utf-8")
    text = f"antes\u2028meio\u2029depois\u0085fim\n{target}"
    joined.rows[0]["message"]["content"] = text
    raw = newline.join(json.dumps(row, ensure_ascii=False).encode("utf-8") for row in joined.rows) + newline
    joined.original.write_bytes(raw)
    joined.snapshot.write_bytes(raw)
    converted = convert_snapshot(joined.snapshot)
    joined.record = replace(joined.record, source=ConversationSource(str(joined.snapshot), "claude",
        converted.source_digest, tuple(sorted(converted.selected_uuids))))
    assert b"\xe2\x80\xa8" in raw and b"\xe2\x80\xa9" in raw and b"\xc2\x85" in raw
    assert history.source_rows(joined.record.source)[0]["message"]["content"] == text
    assert compose(joined)[0].text == text
    monkeypatch.setattr(history, "transfer_for_session", lambda *args, **kwargs: joined.record)
    client, headers = client_for(joined, monkeypatch)
    response = client.get("/api/sessions/s/history", headers=headers)
    assert response.status_code == 200 and response.json()[0]["text"] == text
    joined.snapshot.write_bytes(raw[:-1])
    incomplete = replace(joined.record.source, digest=hashlib.sha256(raw[:-1]).hexdigest())
    with pytest.raises(history.HistoryError, match="linha incompleta"):
        history.source_rows(incomplete)
