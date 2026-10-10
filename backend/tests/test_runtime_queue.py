import json
from pathlib import Path

import pytest

from app.runtime_queue import QueueStore, initial_state

CLOCK = {"monotonic_s": 10.0, "epoch_s": 1800000000.0}


def open_store(tmp_path, rows=None):
    return QueueStore(tmp_path / "key.queue-state.json", tmp_path / "projection",
                      initial_state("key", 1, "session", rows or []))


def append(text="Olá", entry_id="entry-1"):
    return {"kind": "append", "text": text, "entry_id": entry_id,
            "delivered": False, "ts": None, "pre_transcript": False}


def test_local_command_answer_confirms_the_command_outside_the_transcript(tmp_path):
    """A CLI responde `/btw` sozinha e não grava a linha: sem isto a bolha esperava para sempre."""
    store = open_store(tmp_path)
    store.exec(1, "a0", CLOCK, {**append("btw", "e0"), "delivered": True})
    store.exec(1, "a1", CLOCK, {**append("/btw", "e1"), "delivered": True})
    store.exec(1, "a2", CLOCK, {**append("/context", "e2"), "delivered": True})
    store.exec(1, "local", CLOCK, {"kind": "append_local", "text": "/btw isn't available in this environment.",
                                   "entry_id": "l1", "confirms": "/btw"})
    rows = {r["id"]: r for r in store.state["rows"]}
    assert rows["e1"]["confirmed"]
    assert not rows["e0"].get("confirmed") and not rows["e2"].get("confirmed")


def test_phase_receipts_do_not_copy_a_large_intent(tmp_path):
    """Mesma regra de phase_receipts_do_not_copy_a_large_intent em runtime_queue.rs."""
    store = open_store(tmp_path)
    big = "x" * 100_000
    store.exec(1, "prepare", CLOCK, {"kind": "prepare", "id": "op", "payload": {"kind": "input", "frame": {"text": big}},
                                     "entry_id": None})
    store.exec(1, "cursor", CLOCK, {"kind": "bind_dispatch", "id": "op", "cursor": cursor_at(1)})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "op", "wire_id": "wire:op:1"})
    finish = {"kind": "finish", "id": "op", "status": "accepted",
              "result": {"operation_id": "op", "disposition": "accepted", "payload": {"tool_result": big}}}
    first = store.exec(1, "finish", CLOCK, finish)
    assert first["payload"]["frame"]["text"] == big
    for call in ["call::prepare", "call::cursor", "call::dispatch", "call::finish"]:
        assert len(json.dumps(store.state["operations"][call]["result"])) < 1_000, call
    assert (tmp_path / "key.queue-state.json").stat().st_size < 250_000
    replay = store.exec(1, "finish", CLOCK, finish)
    assert [replay[k] for k in ("id", "status", "entry_id")] == [first[k] for k in ("id", "status", "entry_id")]
    assert replay["result"]["disposition"] == "accepted" and replay["result"]["operation_id"] == "op"
    with pytest.raises(ValueError):
        store.exec(1, "finish", CLOCK, {**finish, "status": "rejected", "result": None})


def test_same_operation_does_not_append_twice(tmp_path):
    store = open_store(tmp_path)
    first = store.exec(1, "call-1", CLOCK, append())
    assert store.exec(1, "call-1", CLOCK, append()) == first
    assert len(store.state["rows"]) == 1
    with pytest.raises(ValueError):
        store.exec(1, "call-1", CLOCK, append("Outro texto"))


def test_state_before_projection(tmp_path, monkeypatch):
    store = open_store(tmp_path)
    original = store.ensure_projection
    monkeypatch.setattr(store, "ensure_projection", lambda: (_ for _ in ()).throw(OSError("projection")))
    with pytest.raises(OSError):
        store.exec(1, "call-1", CLOCK, append())
    assert len(json.loads(store.state_path.read_text())["rows"]) == 1
    monkeypatch.setattr(store, "ensure_projection", original)
    row = store.exec(1, "call-1", CLOCK, append())
    assert row["id"] == "entry-1"
    assert len(store.state["rows"]) == 1
    assert json.loads((store.projection_dir / "session.jsonl").read_text()) == row


def test_corrupt_state_is_not_empty_queue(tmp_path):
    store = open_store(tmp_path)
    store.state_path.write_text("{")
    with pytest.raises(ValueError):
        open_store(tmp_path)


def test_unknown_never_unclaims(tmp_path):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    store.exec(1, "prepare", CLOCK, {"kind": "prepare", "id": "op", "payload": {"text": "Olá"}, "entry_id": "entry-1"})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "op", "wire_id": "wire:op:1"})
    store.exec(1, "unknown", CLOCK, {"kind": "finish", "id": "op", "status": "unknown", "result": None})
    with pytest.raises(ValueError):
        store.exec(1, "unclaim", CLOCK, {"kind": "set_delivered", "entry_id": "entry-1", "value": False, "steered": False})
    assert store.state["rows"][0]["delivered"] is True
    assert not store.state["rows"][0].get("confirmed")


def test_accepted_but_unconfirmed_goes_back_to_the_queue(tmp_path):
    """A troca de conta devolve à fila o que a CLI aceitou e nunca gravou: `accepted` não é incerto."""
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, {**append(), "delivered": True})
    store.exec(1, "prepare", CLOCK, {"kind": "prepare", "id": "op", "payload": {"text": "Olá"}, "entry_id": "entry-1"})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "op", "wire_id": "wire:op:1"})
    store.exec(1, "accepted", CLOCK, {"kind": "finish", "id": "op", "status": "accepted", "result": None})
    assert store.state["rows"][0]["delivered"] is True
    store.exec(1, "unclaim", CLOCK, {"kind": "set_delivered", "entry_id": "entry-1", "value": False, "steered": False})
    assert store.state["rows"][0]["delivered"] is False


def test_cap_keeps_pending(tmp_path):
    rows = [{"id": str(i), "text": "pending", "ts": 1, "delivered": True, "desistiu": True} for i in range(1000)]
    store = open_store(tmp_path, rows)
    with pytest.raises(ValueError):
        store.exec(1, "new", CLOCK, append())
    assert store.state["rows"] == rows


def test_rename_keeps_key(tmp_path):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    store.exec(1, "rename", CLOCK, {"kind": "rename", "name": "new-name"})
    assert store.state["owner_key"] == "key"
    assert store.state_path.name == "key.queue-state.json"
    assert not (store.projection_dir / "session.jsonl").exists()
    assert (store.projection_dir / "new-name.jsonl").exists()


def test_unknown_reply_cannot_downgrade_final(tmp_path):
    store = open_store(tmp_path)
    store.exec(1, "prepare", CLOCK, {"kind": "prepare", "id": "op", "payload": {}, "entry_id": None})
    store.exec(1, "accepted", CLOCK, {"kind": "finish", "id": "op", "status": "accepted", "result": {"ok": True}})
    store.exec(1, "late", CLOCK, {"kind": "finish", "id": "op", "status": "unknown", "result": None})
    assert store.state["operations"]["op"]["status"] == "accepted"
    assert store.state["operations"]["op"]["result"] == {"ok": True}


def test_wrong_generation_cannot_mutate(tmp_path):
    store = open_store(tmp_path)
    with pytest.raises(ValueError):
        store.exec(2, "append", CLOCK, append())
    assert store.state["rows"] == []


def test_recovery_keeps_uncertain_dispatch_visible(tmp_path):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    store.exec(1, "prepare", CLOCK, {"kind": "prepare", "id": "op", "payload": {}, "entry_id": "entry-1"})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "op", "wire_id": "wire:op:1"})
    store = open_store(tmp_path)
    store.exec(1, "recover", CLOCK, {"kind": "recover"})
    assert store.state["operations"]["op"]["status"] == "unknown"
    assert store.state["rows"][0]["delivered"] is True


def test_fsync_after_rename_fences_store(tmp_path, monkeypatch):
    store = open_store(tmp_path)
    original = store._atomic

    def fail_after_commit(path, data):
        original(path, data)
        if path == store.state_path:
            raise OSError("directory fsync")

    monkeypatch.setattr(store, "_atomic", fail_after_commit)
    with pytest.raises(OSError):
        store.exec(1, "call-1", CLOCK, append())
    assert json.loads(store.state_path.read_text())["rows"][0]["id"] == "entry-1"
    with pytest.raises(OSError):
        store.exec(1, "call-2", CLOCK, append(entry_id="entry-2"))
    monkeypatch.setattr(store, "_atomic", original)
    store.exec(1, "repair", CLOCK, {"kind": "ensure_projection"})
    assert store.exec(1, "call-1", CLOCK, append())["id"] == "entry-1"
    assert len(store.state["rows"]) == 1


@pytest.mark.parametrize("generation, request_id", [(2, 1), (1, "1"), (1, True)])
def test_late_reply_matches_generation_and_type(tmp_path, generation, request_id):
    store = open_store(tmp_path)
    store.exec(1, "prepare", CLOCK, {"kind": "prepare", "id": "op", "payload": {"request_id": 1}, "entry_id": None})
    store.exec(1, "begin", CLOCK, {"kind": "begin_dispatch", "id": "op", "wire_id": "wire:1"})
    with pytest.raises(ValueError):
        store.exec(1, "late", CLOCK, {"kind": "late_rpc_resolution", "id": "op", "wire_id": "wire:1",
                                     "generation": generation, "request_id": request_id, "result": {"ok": True}})
    assert store.state["operations"]["op"]["status"] == "dispatching"


def test_facade_and_reserve_share_authoritative_state(tmp_path, monkeypatch):
    from contextlib import contextmanager
    from app import runtime_queue
    from app.pqueue import PromptQueue
    store = open_store(tmp_path)
    monkeypatch.setattr("app.pqueue._queue_dir", lambda: store.projection_dir)

    class Coordinator:
        @contextmanager
        def queue_gate(self, name):
            yield store

        def queue_rpc(self, route, call_id, clock, action):
            return route.exec(1, call_id, clock, action)

        def settle_before_queue(self, name):
            pass

    runtime_queue.configure(Coordinator())
    try:
        queue = PromptQueue("session")
        row = queue.append("Fila gerenciada")
        assert queue.load() == [row]
        assert queue.claim_undelivered() == [{**row, "delivered": True}]
        assert queue.entry_delivered(row["id"]) is True
        assert queue.bump_attempts(row["id"]) == 1
        queue.set_delivered(row["id"], False)
        assert queue.entry_delivered(row["id"]) is False
        queue.desistir(row["id"])
        assert queue.remove(row["id"]) is True
        local = queue.append_saida_local("Saída local")
        assert local["confirmed"] is True
        queue.rename("renamed")
        assert store.state["name"] == "renamed"
        queue.clear()
        assert queue.load() == []
    finally:
        runtime_queue.configure(None)


def test_distinct_occurrence_is_committed_with_confirmation(tmp_path):
    from app.runtime_receipt import ReceiptIndex
    store = open_store(tmp_path)
    transcript = tmp_path / "chat.jsonl"
    transcript.touch()
    index = ReceiptIndex("claude", "sid")
    cursor = index.capture(transcript)
    for operation_id in ("op-1", "op-2"):
        row_id = "entry-" + operation_id
        store.exec(1, "append-" + operation_id, CLOCK, append(entry_id=row_id))
        store.exec(1, "prepare-" + operation_id, CLOCK, {"kind": "prepare", "id": operation_id, "payload": {}, "entry_id": row_id})
        store.exec(1, "bind-" + operation_id, CLOCK, {"kind": "bind_dispatch", "id": operation_id, "cursor": cursor})
        store.exec(1, "begin-" + operation_id, CLOCK, {"kind": "begin_dispatch", "id": operation_id, "wire_id": "wire:" + operation_id})
    transcript.write_text('{"type":"user","uuid":"echo-1","message":{"content":"Olá"}}\n')
    index.scan(transcript)
    proof = index.match_after(cursor, store.state["rows"][0], {})
    assert store.exec(1, "confirm-1", CLOCK, {"kind": "confirm_occurrence", "id": "op-1", "proof": proof}) is True
    assert store.exec(1, "confirm-2", CLOCK, {"kind": "confirm_occurrence", "id": "op-2", "proof": proof}) is False
    store = open_store(tmp_path)
    assert sum(bool(row.get("confirmed")) for row in store.state["rows"]) == 1
    assert len(store.state["used_occurrences"]) == 1


@pytest.mark.parametrize("status,side_effect,terminal_claim,expected", [
    (None, False, True, False), ("prepared", False, True, False),
    ("unknown", False, True, True), ("dispatching", False, True, True),
    ("accepted", False, True, True), ("confirmed", False, True, True),
    ("prepared", True, True, True), (None, False, False, True),
])
def test_terminal_claim_recovery_requires_proof_before_dispatch(tmp_path, status, side_effect, terminal_claim, expected):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    store.exec(1, "terminal:queue:999" if terminal_claim else "legacy-claim", CLOCK,
               {"kind": "claim", "min_ts": 1.0, "limit": 1, "entry_id": None})
    if status:
        store.exec(1, "root", CLOCK, {"kind": "prepare", "id": "root", "payload": {"kind": "input"}, "entry_id": "entry-1"})
        if status == "dispatching":
            store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "root", "wire_id": "wire"})
        elif status != "prepared":
            store.exec(1, "finish", CLOCK, {"kind": "finish", "id": "root", "status": status, "result": {}})
    if side_effect:
        store.exec(1, "phase", CLOCK, {"kind": "prepare", "id": "phase", "payload": {"logical_id": "root"}, "entry_id": None})
        store.exec(1, "phase-dispatch", CLOCK, {"kind": "begin_dispatch", "id": "phase", "wire_id": "rpc"})
    store = open_store(tmp_path)
    store.exec(1, "recover", CLOCK, {"kind": "recover"})
    assert store.state["rows"][0]["delivered"] is expected


@pytest.mark.parametrize("claimed", [False, True])
@pytest.mark.parametrize("attempts", [0, 1, 2])
@pytest.mark.parametrize("rejected", [False, True])
def test_terminal_finish_commits_counter_and_row_atomically(tmp_path, claimed, attempts, rejected):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    for number in range(attempts):
        store.exec(1, f"bump:{number}", CLOCK, {"kind": "bump_attempts", "entry_id": "entry-1"})
    if claimed:
        store.exec(1, "terminal:queue:999", CLOCK, {"kind": "claim", "min_ts": 1.0, "limit": 1, "entry_id": None})
    store.exec(1, "terminal:queue:1000", CLOCK, {"kind": "prepare", "id": "attempt", "entry_id": "entry-1",
        "payload": {"operation_id": "attempt", "kind": "input", "payload": {"text": "Olá", "pre_transcript": False, "_terminal_generation": 1}}})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "attempt", "wire_id": "terminal:1:attempt"})
    status = "rejected" if rejected else "deferred"
    result = {"operation_id": "attempt", "disposition": status, "payload": {"cleanup": "proved"}}
    finish = {"kind": "finish", "id": "attempt", "status": status, "result": result}
    store.exec(1, "finish", CLOCK, finish)
    abandoned = rejected or attempts == 2
    assert store.state["rows"][0]["delivered"] is abandoned
    if abandoned:
        assert store.state["rows"][0]["desistiu"] is True
    else:
        assert store.state["rows"][0]["attempts"] == attempts + 1
    row = dict(store.state["rows"][0])
    store = open_store(tmp_path)
    store.exec(1, "recover1", CLOCK, {"kind": "recover"})
    store.exec(1, "finish-new-id", CLOCK, finish)
    store.exec(1, "recover2", CLOCK, {"kind": "recover"})
    assert store.state["rows"][0] == row


def test_terminal_prepare_missing_row_is_recovered_with_same_body(tmp_path):
    store = open_store(tmp_path)
    payload = {"operation_id": "missing", "kind": "input", "payload": {
        "text": "Olá 🌎 — 📎 imagem: /tmp/x.png", "pre_transcript": True, "_terminal_generation": 1}}
    store.exec(1, "terminal:queue:1", CLOCK, {"kind": "prepare", "id": "missing", "payload": payload, "entry_id": "missing"})
    store = open_store(tmp_path)
    store.exec(1, "recover1", CLOCK, {"kind": "recover"})
    assert len(store.state["rows"]) == 1
    row = store.state["rows"][0]
    assert row["id"] == "missing"
    assert row["text"] == payload["payload"]["text"]
    assert row["pre_transcript"] is True
    assert row["delivered"] is False
    store.exec(1, "recover2", CLOCK, {"kind": "recover"})
    assert store.state["rows"] == [row]


@pytest.mark.parametrize("claimed", [False, True])
@pytest.mark.parametrize("attempts", [0, 1, 2])
@pytest.mark.parametrize("rejected", [False, True])
def test_terminal_legacy_finish_recovery_is_idempotent(tmp_path, claimed, attempts, rejected):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    if claimed:
        store.exec(1, "terminal:queue:1000", CLOCK, {"kind": "claim", "min_ts": 1.0, "limit": 1, "entry_id": None})
    store.exec(1, "terminal:queue:1001", CLOCK, {"kind": "prepare", "id": "attempt", "entry_id": "entry-1",
        "payload": {"operation_id": "attempt", "kind": "input", "payload": {"text": "Olá", "pre_transcript": False}}})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "attempt", "wire_id": "terminal:1:attempt"})
    frozen = json.loads(store.state_path.read_text())
    frozen["rows"][0]["attempts"] = attempts
    status = "rejected" if rejected else "deferred"
    frozen["operations"]["attempt"].update(status=status, result={"operation_id": "attempt", "disposition": status, "payload": {"cleanup": "proved"}})
    store.state_path.write_text(json.dumps(frozen))
    store = open_store(tmp_path)
    store.exec(1, "recover1", CLOCK, {"kind": "recover"})
    row = dict(store.state["rows"][0])
    assert row["delivered"] is (rejected or attempts == 2)
    if rejected or attempts == 2:
        assert row["desistiu"] is True
    else:
        assert row["attempts"] == attempts + 1
    store.exec(1, "recover2", CLOCK, {"kind": "recover"})
    assert store.state["rows"][0] == row


@pytest.mark.parametrize("status", ["unknown", "dispatching", "accepted", "confirmed", "prepared"])
@pytest.mark.parametrize("marker", [False, True])
def test_missing_row_never_reconstructed_for_effect_or_headless(tmp_path, status, marker):
    store = open_store(tmp_path)
    payload = {"operation_id": "root", "kind": "input", "payload": {"text": "Olá", "pre_transcript": True}}
    if marker:
        payload["payload"]["_terminal_generation"] = 1
    store.exec(1, "headless-prepare", CLOCK, {"kind": "prepare", "id": "root", "entry_id": "entry-1", "payload": payload})
    if status == "dispatching":
        store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "root", "wire_id": "wire"})
    elif status != "prepared":
        store.exec(1, "finish", CLOCK, {"kind": "finish", "id": "root", "status": status, "result": {}})
    if status == "prepared":
        store.exec(1, "phase", CLOCK, {"kind": "prepare", "id": "phase", "entry_id": None, "payload": {"logical_id": "root"}})
        store.exec(1, "phase-dispatch", CLOCK, {"kind": "begin_dispatch", "id": "phase", "wire_id": "wire"})
    store.exec(1, "recover", CLOCK, {"kind": "recover"})
    assert store.state["rows"] == []


@pytest.mark.parametrize("transition", ["after_bump", "after_unclaim", "after_abandon"])
def test_legacy_terminal_finish_completed_parts_are_not_counted_twice(tmp_path, transition):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    store.exec(1, "terminal:queue:1", CLOCK, {"kind": "prepare", "id": "attempt", "entry_id": "entry-1", "payload": {
        "operation_id": "attempt", "kind": "input", "payload": {"text": "Olá", "pre_transcript": False}}})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "attempt", "wire_id": "terminal:1:attempt"})
    frozen = json.loads(store.state_path.read_text())
    from app.runtime_queue import _operation
    status = "rejected" if transition == "after_abandon" else "deferred"
    result = {"operation_id": "attempt", "disposition": status, "payload": {"cleanup": "proved"}}
    frozen["operations"]["attempt"].update(status=status, result=result)
    finish = {"kind": "finish", "id": "attempt", "status": status, "result": result}
    frozen["operations"]["call::terminal:queue:2"] = _operation("call::terminal:queue:2", finish)
    frozen["operations"]["call::terminal:queue:2"].update(status="accepted", result={})
    frozen["rows"][0]["attempts"] = 1
    if transition == "after_unclaim":
        frozen["rows"][0]["delivered"] = False
    elif transition == "after_abandon":
        frozen["rows"][0].update(desistiu=True, desistiu_ts=CLOCK["epoch_s"] - 10)
    if transition != "after_abandon":
        bump = {"kind": "bump_attempts", "entry_id": "entry-1"}
        frozen["operations"]["call::terminal:queue:3"] = _operation("call::terminal:queue:3", bump)
        frozen["operations"]["call::terminal:queue:3"].update(status="accepted", result=1)
    store.state_path.write_text(json.dumps(frozen))
    store = open_store(tmp_path)
    store.exec(1, "recover", CLOCK, {"kind": "recover"})
    row = store.state["rows"][0]
    assert row["attempts"] == 1
    if transition == "after_abandon":
        assert row["desistiu_ts"] == CLOCK["epoch_s"] - 10
    else:
        assert row["delivered"] is False


def test_terminal_rejected_before_dispatch_abandons_pending_row(tmp_path):
    store = open_store(tmp_path)
    store.exec(1, "append", CLOCK, append())
    store.exec(1, "prepare", CLOCK, {"kind": "prepare", "id": "op", "entry_id": "entry-1", "payload": {
        "operation_id": "op", "kind": "input", "payload": {"text": "Olá", "_terminal_generation": 1}}})
    store.exec(1, "finish", CLOCK, {"kind": "finish", "id": "op", "status": "rejected", "result": {
        "operation_id": "op", "disposition": "rejected", "payload": {"code": "refused"}}})
    assert store.state["rows"][0]["delivered"] is True
    assert store.state["rows"][0]["desistiu"] is True


@pytest.mark.parametrize("exhausted", [False, True])
def test_recovered_terminal_row_removed_after_failure_stays_removed(tmp_path, exhausted):
    store = open_store(tmp_path)
    store.exec(1, "prepare-root", CLOCK, {"kind": "prepare", "id": "root", "entry_id": "entry-1", "payload": {
        "operation_id": "root", "kind": "input", "payload": {"text": "Olá 🌎", "pre_transcript": True, "_terminal_generation": 1}}})
    store.exec(1, "recover-first-creation", CLOCK, {"kind": "recover"})
    assert len(store.state["rows"]) == 1
    if exhausted:
        for n in range(2):
            store.exec(1, f"bump:{n}", CLOCK, {"kind": "bump_attempts", "entry_id": "entry-1"})
    store.exec(1, "prepare-attempt", CLOCK, {"kind": "prepare", "id": "attempt", "entry_id": "entry-1", "payload": {
        "operation_id": "attempt", "kind": "input", "payload": {"text": "Olá 🌎", "_terminal_generation": 1}}})
    store.exec(1, "dispatch", CLOCK, {"kind": "begin_dispatch", "id": "attempt", "wire_id": "terminal:1:attempt"})
    status = "deferred" if exhausted else "rejected"
    store.exec(1, "finish", CLOCK, {"kind": "finish", "id": "attempt", "status": status, "result": {
        "operation_id": "attempt", "disposition": status, "payload": {"cleanup": "proved"}}})
    assert store.state["rows"][0]["desistiu"] is True
    assert store.exec(1, "remove", CLOCK, {"kind": "remove", "entry_id": "entry-1"}) is True
    store = open_store(tmp_path)
    for n in range(2):
        store.exec(1, f"recover-again:{n}", CLOCK, {"kind": "recover"})
        assert store.state["rows"] == []


# --- Poda do diário: tamanho limitado sem reenvio nem confirmação dupla ---

FIXTURE = Path(__file__).parent / "fixtures" / "runtime_queue" / "compaction.json"
PARITY_FILL = 270


def fill(store, count, prefix="fill"):
    for index in range(count):
        store.exec(1, f"{prefix}:{index}", CLOCK, {"kind": "set_runtime_state", "state": {}})


def cursor_at(offset):
    return {"conversation": "c", "file_identity": "1:2", "offset": offset, "anchor": "a"}


def proof_at(cursor, offset, text="Olá mundo"):
    return {"cursor": cursor, "occurrence": {"id": "c|1:2|id:echo", "conversation": "c", "file_identity": "1:2",
            "offset": offset, "end_offset": offset + 10, "text": text, "kind": "user", "timestamp": None},
            "normalized_text": text, "observed_anchor": cursor["anchor"]}


def dispatch(operation_id, entry_id, cursor, *, text="Olá mundo", finish="accepted", reply=None):
    """Uma mensagem como o ator manda: raiz, fase de fio, cursor, envio e resposta."""
    wire = f"wire:{operation_id}:1"
    calls = []
    if entry_id is not None:
        calls.append({"kind": "append", "text": text, "delivered": False, "ts": None, "pre_transcript": False, "entry_id": entry_id})
    calls += [
        {"kind": "prepare", "id": operation_id, "payload": {"operation_id": operation_id, "kind": "input",
            "payload": {"text": text}}, "entry_id": entry_id},
        {"kind": "prepare", "id": wire, "payload": {"logical_id": operation_id, "frame": {"text": text}}, "entry_id": entry_id},
        {"kind": "bind_dispatch", "id": wire, "cursor": cursor},
        {"kind": "bind_dispatch", "id": operation_id, "cursor": cursor},
        {"kind": "begin_dispatch", "id": wire, "wire_id": wire},
        {"kind": "begin_dispatch", "id": operation_id, "wire_id": wire},
    ]
    if finish:
        calls += [{"kind": "finish", "id": wire, "status": finish, "result": {"write_outcome": "written"}},
                  {"kind": "finish", "id": operation_id, "status": finish, "result": {"operation_id": operation_id,
                      "disposition": finish, "payload": {"answer": "resposta"} if reply is None else reply}}]
    return [[f"{operation_id}:{index}", action] for index, action in enumerate(calls)]


def run(store, steps):
    return [store.exec(1, call_id, CLOCK, action) for call_id, action in steps]


def test_state_stays_bounded_over_2000_messages_with_100kb_replies(tmp_path, monkeypatch):
    from app import runtime_queue
    monkeypatch.setattr(runtime_queue, "_RECENT_CALLS", 32)
    monkeypatch.setattr(runtime_queue.os, "fsync", lambda fd: None)
    store = open_store(tmp_path)
    big = {"tool_result": "x" * 100_000}
    for index in range(1, 2001):
        run(store, dispatch(f"op-{index}", None, cursor_at(index), reply=big))
    receipts = [key for key in store.state["operations"] if key.startswith("call::")]
    assert len(receipts) == 32 and len(store.state["operations"]) - len(receipts) <= 32
    assert store.state_path.stat().st_size < 1_000_000
    assert "x" * 1000 not in store.state_path.read_text()


def test_kept_reply_keeps_its_disposition(tmp_path):
    store = open_store(tmp_path)
    run(store, dispatch("op", "entry", cursor_at(0), reply={"tool_result": "x" * 1000}))
    kept = store.state["operations"]["op"]
    # O formato de RuntimeReply continua: quem repete a operação recebe a resposta guardada.
    assert kept["status"] == "accepted"
    assert kept["result"] == {"operation_id": "op", "disposition": "accepted", "payload": None}
    assert store.state["operations"]["call::op:8"]["payload"]["result"]["payload"] is None
    with pytest.raises(ValueError):
        store.exec(1, "op:8", CLOCK, {"kind": "finish", "id": "op", "status": "rejected", "result": {}})


def test_final_operation_of_open_row_survives_and_is_not_redispatched(tmp_path, monkeypatch):
    from app import runtime_queue
    monkeypatch.setattr(runtime_queue, "_RECENT_CALLS", 8)
    store = open_store(tmp_path)
    run(store, dispatch("op", "entry", cursor_at(0)))
    fill(store, 20)
    # Entrada ainda sem recibo: raiz e fase ficam, e preparar de novo devolve o aceito.
    assert {"op", "wire:op:1"} <= set(store.state["operations"])
    again = store.exec(1, "retry", CLOCK, {"kind": "prepare", "id": "op", "payload": {"operation_id": "op",
        "kind": "input", "payload": {"text": "Olá mundo"}}, "entry_id": "entry"})
    assert again["status"] == "accepted"
    # Confirmada, sai do diário; a linha confirmada não volta a ser reclamada para envio.
    store.exec(1, "confirm", CLOCK, {"kind": "confirm", "entry_ids": ["entry"]})
    fill(store, 20, "after")
    assert not {"op", "wire:op:1"} & set(store.state["operations"])
    assert store.exec(1, "claim", CLOCK, {"kind": "claim", "min_ts": 0, "limit": None, "entry_id": None}) == []


def test_uncertain_operation_and_its_phases_are_never_pruned(tmp_path, monkeypatch):
    from app import runtime_queue
    monkeypatch.setattr(runtime_queue, "_RECENT_CALLS", 8)
    store = open_store(tmp_path)
    run(store, dispatch("ctl", None, None, finish=None)
        + [["phase-done", {"kind": "finish", "id": "wire:ctl:1", "status": "accepted", "result": {}}]])
    run(store, dispatch("sent", "entry", cursor_at(0), finish=None))
    store.exec(1, "recover", CLOCK, {"kind": "recover"})
    fill(store, 40)
    operations = store.state["operations"]
    assert operations["ctl"]["status"] == "unknown" and operations["wire:ctl:1"]["status"] == "accepted"
    assert operations["sent"]["status"] == "unknown"
    with pytest.raises(ValueError):
        store.exec(1, "unclaim", CLOCK, {"kind": "set_delivered", "entry_id": "entry", "value": False, "steered": False})


def test_recent_receipt_replays_and_old_one_is_pruned(tmp_path, monkeypatch):
    from app import runtime_queue
    monkeypatch.setattr(runtime_queue, "_RECENT_CALLS", 8)
    store = open_store(tmp_path)
    first = store.exec(1, "same", CLOCK, append())
    fill(store, 7)
    assert store.exec(1, "same", CLOCK, append()) == first
    assert len(store.state["rows"]) == 1
    fill(store, 8, "more")
    assert "call::same" not in store.state["operations"]


def test_pruned_operation_never_frees_its_occurrence_for_another(tmp_path, monkeypatch):
    from app import runtime_queue
    from app.runtime_receipt import ReceiptIndex
    monkeypatch.setattr(runtime_queue, "_RECENT_CALLS", 8)
    store = open_store(tmp_path)
    transcript = tmp_path / "chat.jsonl"
    transcript.touch()
    index = ReceiptIndex("claude", "sid")
    cursor = index.capture(transcript)
    run(store, dispatch("first", "entry-1", cursor, text="Olá"))
    run(store, dispatch("second", "entry-2", cursor, text="Olá"))
    transcript.write_text('{"type":"user","uuid":"echo-1","message":{"content":"Olá"}}\n')
    index.scan(transcript)
    proof = index.match_after(cursor, store.state["rows"][0], store.state["used_occurrences"])
    assert store.exec(1, "proof-1", CLOCK, {"kind": "confirm_occurrence", "id": "first", "proof": proof}) is True
    fill(store, 20)
    assert "first" not in store.state["operations"]
    # A segunda, sem recibo e com cursor antes do eco, ainda poderia casá-lo: o uso fica.
    assert list(store.state["used_occurrences"]) == [proof["occurrence"]["id"]]
    assert index.match_after(cursor, store.state["rows"][1], store.state["used_occurrences"]) is None
    assert store.exec(1, "proof-2", CLOCK, {"kind": "confirm_occurrence", "id": "second", "proof": proof}) is False
    assert not store.state["rows"][1].get("confirmed")
    # Sem ninguém que possa casá-lo, o uso sai; cursor novo já nasce depois do eco.
    store.exec(1, "confirm", CLOCK, {"kind": "confirm", "entry_ids": ["entry-2"]})
    fill(store, 20, "after")
    assert store.state["used_occurrences"] == {}
    assert index.match_after(index.capture(transcript), {"id": "x", "text": "Olá"}, {}) is None


def test_v1_state_shrinks_on_first_open(tmp_path, monkeypatch):
    from app import runtime_queue
    monkeypatch.setattr(runtime_queue, "_RECENT_CALLS", 8)
    old = {"id": "old", "payload": {}, "entry_id": None, "status": "accepted", "result": None,
           "dispatch_cursor": None, "wire_attempts": {}}
    v1 = {"version": 1, "owner_key": "key", "generation": 1, "name": "session", "rows": [],
          "operations": {"old": old, "stuck": {**old, "id": "stuck", "status": "unknown"},
                         "call::old": {**old, "id": "call::old"}},
          "used_occurrences": {"legacy": {"operation_id": "old", "generation": 1}}, "runtime_state": {}}
    (tmp_path / "key.queue-state.json").write_text(json.dumps(v1))
    store = open_store(tmp_path)
    # Encolhe já na abertura, antes de qualquer gravação nova.
    saved = json.loads(store.state_path.read_text())
    assert saved["version"] == 2 and saved["next_seq"] == 9
    assert set(saved["operations"]) == {"stuck"} and saved["used_occurrences"] == {}
    assert store.state == saved


def _parity_steps():
    # As duas despachadas antes do eco (offset 20): a segunda ainda poderia casá-lo.
    steps = dispatch("first", "entry-1", cursor_at(10))
    steps += dispatch("second", "entry-2", cursor_at(15))
    steps.append(["proof-first", {"kind": "confirm_occurrence", "id": "first", "proof": proof_at(cursor_at(10), 20)}])
    steps += dispatch("system:1:5", None, None)
    steps += dispatch("ctl", None, None, finish=None)
    steps.append(["ctl-phase", {"kind": "finish", "id": "wire:ctl:1", "status": "accepted", "result": {}}])
    return steps


def _parity_run(store):
    results = run(store, _parity_steps())
    fill(store, PARITY_FILL)
    return results


def test_compaction_matches_rust_fixture(tmp_path):
    """O mesmo roteiro roda em crates/hangar-server/tests/it/runtime_queue.rs contra este arquivo."""
    store = open_store(tmp_path)
    results = _parity_run(store)
    expected = json.loads(FIXTURE.read_text(encoding="utf-8"))
    assert expected["steps"] == _parity_steps() and expected["fill"] == PARITY_FILL
    assert json.loads(json.dumps(results)) == expected["results"]
    assert json.loads(json.dumps(store.state)) == expected["final"]
    assert {"second", "wire:second:1", "ctl", "wire:ctl:1"} <= set(store.state["operations"])
    assert not {"first", "wire:first:1", "system:1:5"} & set(store.state["operations"])
    assert len(store.state["used_occurrences"]) == 1


if __name__ == "__main__":
    # Regera o oráculo a partir de backend/: `PYTHONPATH=. uv run python tests/test_runtime_queue.py`.
    import tempfile
    with tempfile.TemporaryDirectory() as directory:
        store = open_store(Path(directory))
        results = _parity_run(store)
        FIXTURE.parent.mkdir(parents=True, exist_ok=True)
        FIXTURE.write_text(json.dumps({"steps": _parity_steps(), "fill": PARITY_FILL, "results": results,
                                       "final": store.state}, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")


def test_queue_read_during_hand_over_uses_projection(tmp_path, monkeypatch):
    # Na adoção pelo Rust o `follow` do chat relia a fila e recebia "sessão em transferência": o
    # SSE fechava. Leitura usa a projeção; escrita continua recusada.
    from contextlib import contextmanager
    from app import runtime_queue
    from app.pqueue import PromptQueue
    from app.runtime_coordinator import TransferInProgress
    store = open_store(tmp_path)
    monkeypatch.setattr("app.pqueue._queue_dir", lambda: store.projection_dir)
    moving = {"on": False}

    class Coordinator:
        @contextmanager
        def queue_gate(self, name):
            if moving["on"]:
                raise TransferInProgress("sessão em transferência; aguarde a posse ser confirmada")
            yield store

        def queue_rpc(self, route, call_id, clock, action):
            return route.exec(1, call_id, clock, action)

        def settle_before_queue(self, name):
            pass

    runtime_queue.configure(Coordinator())
    try:
        queue = PromptQueue("session")
        row = queue.append("Antes da passagem")
        moving["on"] = True
        assert queue.load() == [row]
        with pytest.raises(TransferInProgress):
            queue.append("Durante a passagem")
    finally:
        runtime_queue.configure(None)
