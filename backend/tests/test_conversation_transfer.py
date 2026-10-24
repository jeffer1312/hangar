"""Persistência e identidade da transferência, sem abrir processos de agentes."""
import asyncio
import hashlib
import json
import os
import stat
import uuid
from dataclasses import asdict, replace

import pytest

from app import conversation_transfer as store
from app import registry
from app.adapters.claude_headless import sessions as hl_sessions
from app.adapters.codex import sessions as cx_sessions
from app.conversation_transfer import ConversationSource, ImportBoundary, TransferPhase, TransferRecord
from app.models import SessionInfo


@pytest.fixture(autouse=True)
def isolated(tmp_path, monkeypatch):
    monkeypatch.setattr(store, "_base", lambda: tmp_path / "store")
    monkeypatch.setattr(hl_sessions, "_dir", lambda: tmp_path / "claude-sidecars")
    monkeypatch.setattr(cx_sessions, "_dir", lambda: tmp_path / "codex-sidecars")
    monkeypatch.setattr(store, "session_life", lambda name: "k:old")


def sample_record(tmp_path, *, phase=TransferPhase.SOURCE_STOPPED, source_life="k:old"):
    raw = json.dumps({"type": "user", "uuid": "u0", "parentUuid": None,
                      "message": {"role": "user", "content": "original"}}).encode() + b"\n"
    original = tmp_path / "claude.jsonl"
    original.write_bytes(raw)
    active = tmp_path / "codex.jsonl"
    active.write_bytes(b"")
    return TransferRecord(
        id=str(uuid.uuid4()), name="s", source_life=source_life, phase=phase,
        source=ConversationSource(str(original), "claude", hashlib.sha256(raw).hexdigest(), ("u0",)),
        origin_meta={"name": "s", "cwd": str(tmp_path), "session_id": "original-claude-id", "key": "old",
                     "jsonl": str(original), "headless": True},
        destination_meta={"thread_id": "old-thread", "codex_home": "/tmp/codex",
                          "lifecycle_id": "k:old", "key": "old"},
        boundary=ImportBoundary("old-thread", str(active), 0, ()), error_code=None,
    )


def test_transfer_survives_reload_and_keeps_origin(tmp_path):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    assert store.load_transfer(record.id) == record
    assert store.load_transfer(record.id).origin_meta["session_id"] == "original-claude-id"
    assert store.transfer_active(record.name)


@pytest.mark.parametrize("phase", list(TransferPhase))
def test_all_fields_and_phases_roundtrip(tmp_path, phase):
    record = replace(sample_record(tmp_path, phase=phase), error_code="transfer_restore_failed",
                     boundary=ImportBoundary("old-thread", "/tmp/rollout", 452, ("i0", "i1")))
    store.save_transfer(record)
    assert store.load_transfer(record.id) == record
    assert store.transfer_active(record.name) == (phase not in {
        TransferPhase.COMPLETE, TransferPhase.REJECTED, TransferPhase.ROLLED_BACK})


def test_interrupted_record_commit_keeps_previous_state(tmp_path, monkeypatch):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    substitute = store.atomico.substituir

    def interrupted(src, dst):
        if dst == store._record_path(record.id):
            raise OSError("interrompido")
        substitute(src, dst)

    monkeypatch.setattr(store.atomico, "substituir", interrupted)
    changed = replace(record, phase=TransferPhase.IMPORTED,
                      destination_meta={**record.destination_meta, "thread_id": "new-thread"},
                      boundary=replace(record.boundary, thread_id="new-thread"))
    with pytest.raises(OSError, match="interrompido"):
        store.save_transfer(changed)
    assert store.load_transfer(record.id) == record
    assert store.transfer_for_thread("/tmp/codex", "new-thread") is None
    assert store.transfer_for_thread("/tmp/codex", "old-thread") == record
    assert not list(store._base().rglob("*.tmp"))


def test_interrupted_snapshot_is_not_published(tmp_path, monkeypatch):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    substitute = store.atomico.substituir

    def interrupted(src, dst):
        if dst.suffix == ".jsonl":
            raise OSError("snapshot interrompido")
        substitute(src, dst)

    monkeypatch.setattr(store.atomico, "substituir", interrupted)
    with pytest.raises(OSError, match="snapshot interrompido"):
        store.capture_snapshot(record, record.source.path, ("u0",))
    assert store.load_transfer(record.id) == record
    assert not list(store._base().rglob("*.tmp"))
    assert not list((store._base() / "snapshots").glob("*.jsonl"))


def test_snapshot_keeps_original_bytes_digest_and_permissions(tmp_path):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    original = record.source.path
    captured = store.capture_snapshot(record, original, ("u0",))
    assert captured.phase == TransferPhase.SOURCE_STOPPED
    assert captured.source.path != original
    from pathlib import Path
    assert Path(captured.source.path).read_bytes() == Path(original).read_bytes()
    store.verify_source(captured.source)
    assert store.load_transfer(record.id) == captured
    if os.name != "nt":
        assert stat.S_IMODE(os.stat(captured.source.path).st_mode) == 0o600
        assert stat.S_IMODE(store._record_path(record.id).stat().st_mode) == 0o600
    with open(captured.source.path, "ab") as changed:
        changed.write(b"alterado")
    with pytest.raises(ValueError, match="mudou"):
        store.verify_source(captured.source)


def test_snapshot_requires_confirmed_stop(tmp_path):
    record = sample_record(tmp_path, phase=TransferPhase.PREPARING)
    with pytest.raises(ValueError, match="parada confirmada"):
        store.capture_snapshot(record, record.source.path, ("u0",))
    assert not store._base().exists()


def test_same_name_new_life_does_not_reuse_old_transfer(tmp_path, monkeypatch):
    record = sample_record(tmp_path, phase=TransferPhase.COMPLETE)
    store.save_transfer(record)
    monkeypatch.setattr(store, "session_life", lambda name: "k:new")
    assert store.transfer_for_session(record.name) is None
    assert store.transfer_for_thread("/tmp/codex", "old-thread") == record


def test_complete_belongs_to_destination_life(tmp_path, monkeypatch):
    record = replace(sample_record(tmp_path, phase=TransferPhase.COMPLETE, source_life="t:100"),
                     destination_meta={"codex_home": "/tmp/codex", "thread_id": "old-thread",
                                       "lifecycle_id": "k:destination"})
    store.save_transfer(record)
    monkeypatch.setattr(store, "session_life", lambda name: "k:destination")
    assert store.transfer_for_session("s") == record
    assert not store.transfer_active("s")


def test_transition_marker_survives_technical_life_change(tmp_path):
    record = sample_record(tmp_path, source_life="t:100")
    store.save_transfer(record)
    assert store.transfer_for_session("s", lifecycle_id="t:200", meta={"transfer_id": record.id}) == record
    assert store.transfer_for_session("s", lifecycle_id="t:200", meta={"session_id": "new"}) is None
    assert store.transfer_for_session("s", lifecycle_id="t:100", meta={"session_id": "new"}) is None


def test_thread_index_is_scoped_to_account_and_preserved_after_close(tmp_path):
    record = sample_record(tmp_path, phase=TransferPhase.COMPLETE)
    store.save_transfer(record)
    cx_sessions.save("s", "old-thread", record.boundary.rollout_path, str(tmp_path),
                     codex_home="/tmp/codex", key="old", transfer_id=record.id)
    cx_sessions.delete("s")
    assert store.transfer_for_thread("/tmp/codex", "old-thread") == record
    assert store.transfer_for_thread("/tmp/other-account", "old-thread") is None


def test_clear_drops_active_composition_keeps_archive(tmp_path):
    record = sample_record(tmp_path, phase=TransferPhase.COMPLETE)
    store.save_transfer(record)
    cx_sessions.save("s", "old-thread", record.boundary.rollout_path, str(tmp_path),
                     codex_home="/tmp/codex", key="old", transfer_id=record.id, tool_output_token_limit=144000)
    cx_sessions.update("s", thread_id="after-clear", rollout_path="/tmp/new-rollout")
    assert store.transfer_for_session("s") is None
    assert store.transfer_for_thread("/tmp/codex", "old-thread") == record
    assert cx_sessions.load("s")["key"] == "old"
    assert "transfer_id" not in cx_sessions.load("s")
    assert "tool_output_token_limit" not in cx_sessions.load("s")


def test_terminal_without_key_adopts_once(tmp_path):
    record = sample_record(tmp_path, source_life="t:100")
    record = replace(record, origin_meta={k: v for k, v in record.origin_meta.items() if k != "key"})
    store.save_transfer(record)
    adopted = store.load_transfer(record.id).origin_meta["key"]
    store.save_transfer(replace(record, phase=TransferPhase.RESTORING))
    assert store.load_transfer(record.id).origin_meta["key"] == adopted
    assert store.load_transfer(record.id).source_life == "t:100"


@pytest.mark.parametrize("field,value", [("auth", {}), ("token", "secret"), ("env", {"KEY": "secret"}),
                                          ("config", "secret"), ("password", "secret")])
def test_metadata_cannot_persist_credentials_or_config(tmp_path, field, value):
    record = sample_record(tmp_path)
    with pytest.raises(ValueError, match="não permitidos"):
        store.save_transfer(replace(record, origin_meta={**record.origin_meta, field: value}))
    assert not store._base().exists()


@pytest.mark.parametrize("raw", ["null", "[]", "{bad", '{"id": "invalid"}'])
def test_corruption_is_explicit_not_absent(tmp_path, raw):
    transfer_id = str(uuid.uuid4())
    store._base().mkdir()
    store._record_path(transfer_id).write_text(raw)
    with pytest.raises(ValueError):
        store.load_transfer(transfer_id)
    with pytest.raises(ValueError):
        store.list_incomplete()


def test_record_schema_rejects_wrong_types(tmp_path):
    record = sample_record(tmp_path)
    data = asdict(record)
    data["boundary"]["min_offset"] = True
    store._base().mkdir()
    store._record_path(record.id).write_text(json.dumps(data))
    with pytest.raises(ValueError, match="fronteira"):
        store.load_transfer(record.id)


def test_registry_restores_one_original_card_and_does_not_read_physical_state(tmp_path, monkeypatch):
    record = sample_record(tmp_path, phase=TransferPhase.RESTORE_FAILED)
    record = replace(record, error_code="transfer_restore_failed")
    store.save_transfer(record)
    monkeypatch.setattr(registry, "PairLink", lambda name: type("Link", (), {"get": lambda self: {}})())
    monkeypatch.setattr(registry, "ThenLink", lambda name: type("Link", (), {"get": lambda self: {}})())
    infos = []
    registry._decorate_transfers(infos)
    assert len(infos) == 1
    assert infos[0].name == "s" and infos[0].provider == "claude"
    assert infos[0].jsonl == record.source.path
    assert infos[0].transfer_phase == "restore_failed"
    assert infos[0].problema == "transfer_restore_failed"
    assert infos[0].lifecycle_id == record.source_life

    def forbidden(*args, **kwargs):
        raise AssertionError("não classificar ou aquecer a operação interrompida")

    monkeypatch.setattr(registry.tmux, "capture_pane", forbidden)
    monkeypatch.setattr(registry, "codex_turno_aberto", forbidden)
    result = asyncio.run(registry.SessionRegistry(projects_dir=tmp_path).list_with_state(infos))
    assert result == infos
    assert result[0].state == "idle"


def test_transfer_row_reads_the_group_file(tmp_path, monkeypatch):
    """BRG-29: a linha da transferência leva o grupo lido do arquivo que o Rust grava, par de fora incluído."""
    from app import pair
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    store.save_transfer(sample_record(tmp_path, phase=TransferPhase.RESTORE_FAILED))
    monkeypatch.setattr(registry, "ThenLink", lambda name: type("Link", (), {"get": lambda self: {}})())
    (pair._pair_dir() / "s.json").write_text(json.dumps(
        {"peers": ["b", "ana::y"], "task": "T-1", "gid": "ab12cd34", "harness": {}}), encoding="utf-8")
    (pair._pair_dir() / "external_pairs.json").write_text(json.dumps(
        [{"local_session": "s", "alias": "ana", "peer_session": "y", "peer_owner": "Ana"}]), encoding="utf-8")
    infos = []
    registry._decorate_transfers(infos)
    row = infos[0]
    assert (row.pair_peers, row.pair_gid, row.pair_task) == (["b", "ana::y"], "ab12cd34", "T-1")
    assert row.pair_external == {"alias": "ana", "owner": "Ana", "session": "y"}


def test_registry_recreated_name_does_not_restore_old_operation(tmp_path):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    infos = [SessionInfo(name="s", lifecycle_id="k:new", cwd="/tmp/new")]
    registry._decorate_transfers(infos)
    assert infos[0].transfer_id is None and infos[0].cwd == "/tmp/new"


def test_registry_keeps_one_card_during_sidecar_overlap(tmp_path, monkeypatch):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    cx_sessions.save("s", "old-thread", record.boundary.rollout_path, str(tmp_path),
                     key="old", codex_home="/tmp/codex", transfer_id=record.id)
    monkeypatch.setattr(registry, "PairLink", lambda name: type("Link", (), {"get": lambda self: {}})())
    monkeypatch.setattr(registry, "ThenLink", lambda name: type("Link", (), {"get": lambda self: {}})())
    infos = [SessionInfo(name="s", lifecycle_id="k:old", provider="codex"),
             SessionInfo(name="s", lifecycle_id="k:old", provider="claude")]
    registry._decorate_transfers(infos)
    assert len(infos) == 1 and infos[0].provider == "claude"



def test_live_terminal_same_life_other_transcript_is_not_the_operation(tmp_path):
    record = sample_record(tmp_path, source_life="t:100")
    store.save_transfer(record)
    assert store.transfer_for_session("s", lifecycle_id="t:100", source_path="/tmp/other-id.jsonl") is None
    assert store.transfer_for_session("s", lifecycle_id="t:100", source_path=record.origin_meta["jsonl"]) == record



def test_snapshot_published_but_record_commit_interrupted_keeps_previous_source(tmp_path, monkeypatch):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    substitute = store.atomico.substituir

    def interrupted(src, dst):
        if dst == store._record_path(record.id):
            raise OSError("registro interrompido")
        substitute(src, dst)

    monkeypatch.setattr(store.atomico, "substituir", interrupted)
    with pytest.raises(OSError, match="registro interrompido"):
        store.capture_snapshot(record, record.source.path, ("u0",))
    assert store.load_transfer(record.id).source == record.source
    assert len(list((store._base() / "snapshots").glob("*.jsonl"))) == 1
    assert not list(store._base().rglob("*.tmp"))


def test_index_commit_interrupted_keeps_previous_record(tmp_path, monkeypatch):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    substitute = store.atomico.substituir

    def interrupted(src, dst):
        if dst.parent.name == "indexes":
            raise OSError("índice interrompido")
        substitute(src, dst)

    monkeypatch.setattr(store.atomico, "substituir", interrupted)
    with pytest.raises(OSError, match="índice interrompido"):
        store.save_transfer(replace(record, name="new-name"))
    assert store.load_transfer(record.id) == record
    assert store.transfer_for_session("s") == record


def test_missing_record_alone_is_none(tmp_path):
    assert store.load_transfer(str(uuid.uuid4())) is None
    with pytest.raises(ValueError):
        store.load_transfer("../../secret")


def test_existing_key_and_source_life_cannot_change(tmp_path):
    record = sample_record(tmp_path)
    store.save_transfer(record)
    with pytest.raises(ValueError, match="chave"):
        store.save_transfer(replace(record, origin_meta={**record.origin_meta, "key": "other"}))
    with pytest.raises(ValueError, match="vida"):
        store.save_transfer(replace(record, source_life="k:new"))
    assert store.load_transfer(record.id) == record



def test_parked_source_stays_pending_without_a_physical_session(tmp_path, monkeypatch):
    record = sample_record(tmp_path, source_life="t:100")
    store.save_transfer(record)
    monkeypatch.setattr(store, "session_life", lambda name: None)
    assert store.transfer_active("s")
    assert store.transfer_for_session("s") == record
    # Uma linha existente sem identidade confiável não é apropriada pela recuperação.
    assert store.transfer_for_session("s", lifecycle_id=None) is None
    store.save_transfer(replace(record, phase=TransferPhase.ROLLED_BACK))
    assert not store.transfer_active("s")



def test_incomplete_scan_permission_failure_is_explicit(tmp_path, monkeypatch):
    from pathlib import Path
    original = Path.iterdir

    def denied(path):
        if path == store._base():
            raise PermissionError("sem acesso ao registro privado")
        return original(path)

    monkeypatch.setattr(Path, "iterdir", denied)
    with pytest.raises(PermissionError):
        store.list_incomplete()



def test_snapshot_rejects_a_stale_confirmed_phase(tmp_path):
    record = sample_record(tmp_path)
    store.save_transfer(replace(record, phase=TransferPhase.RESTORING))
    with pytest.raises(ValueError, match="etapa"):
        store.capture_snapshot(record, record.source.path, ("u0",))
    assert not (store._base() / "snapshots").exists()



def test_complete_marker_cannot_borrow_another_account_thread(tmp_path):
    record = sample_record(tmp_path, phase=TransferPhase.COMPLETE)
    store.save_transfer(record)
    other_account = {"provider": "codex", "transfer_id": record.id,
                     "thread_id": "old-thread", "codex_home": "/tmp/other-account"}
    assert store.transfer_for_session("s", lifecycle_id="k:old", meta=other_account) is None



def test_registry_prepared_thread_without_published_marker_keeps_origin(tmp_path, monkeypatch):
    record = sample_record(tmp_path, source_life="t:100")
    store.save_transfer(record)
    cx_sessions.save("s", "old-thread", record.boundary.rollout_path, str(tmp_path),
                     key="old", codex_home="/tmp/codex")
    monkeypatch.setattr(registry, "PairLink", lambda name: type("Link", (), {"get": lambda self: {}})())
    monkeypatch.setattr(registry, "ThenLink", lambda name: type("Link", (), {"get": lambda self: {}})())
    infos = [SessionInfo(name="s", lifecycle_id="k:old", provider="codex", codex_home="/tmp/codex")]
    registry._decorate_transfers(infos)
    assert len(infos) == 1 and infos[0].provider == "claude"
    assert infos[0].transfer_id == record.id and infos[0].lifecycle_id == "t:100"


def test_boundary_digest_roundtrips_and_legacy_missing_digest_is_accepted(tmp_path):
    record = sample_record(tmp_path)
    record = replace(record, boundary=replace(record.boundary, prefix_digest="a" * 64))
    store.save_transfer(record)
    assert store.load_transfer(record.id).boundary.prefix_digest == "a" * 64
    path = store._record_path(record.id)
    legacy = json.loads(path.read_text())
    legacy["boundary"].pop("prefix_digest")
    path.write_text(json.dumps(legacy))
    assert store.load_transfer(record.id).boundary.prefix_digest is None


@pytest.mark.parametrize("digest", ["short", "z" * 64, 1])
def test_boundary_rejects_invalid_digest(tmp_path, digest):
    record = sample_record(tmp_path)
    with pytest.raises(ValueError, match="digest da fronteira"):
        store.save_transfer(replace(record, boundary=replace(record.boundary, prefix_digest=digest)))


def test_transfer_error_has_one_safe_public_contract():
    from app.adapters.codex.transfer import TransferError as importer_error
    assert importer_error is store.TransferError
    error = store.TransferError("session_transfer_model_capacity_unknown", status=422,
                                params={"model": "test-model"})
    assert (error.code, error.status, error.params) == (
        "session_transfer_model_capacity_unknown", 422, {"model": "test-model"})
    with pytest.raises(ValueError):
        store.TransferError("session_transfer_failed", params={"payload": "private"})
