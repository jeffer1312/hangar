import asyncio
import copy
import json
from contextlib import closing
from concurrent.futures import ThreadPoolExecutor

import pytest

from app import runtime_queue as rq, runtime_terminal as terminal, terminal_input as ti
from test_runtime_queue import CLOCK, open_store, fill
from test_runtime_terminal import isolated_owner, live_owner


def intent(operation='root', entry='entry'):
    return {'kind': 'prepare', 'id': operation, 'entry_id': entry, 'payload': {
        'operation_id': operation, 'kind': 'input',
        'payload': {'text': 'fixture-input', '_terminal_generation': 1}}}


def native_payload():
    return {'native': True, 'message_id': 'native-id', 'native_status': 'delivered',
        'cleanup': 'not_needed', 'code': 'native_write', 'stage': 'native',
        'preserve_binding': True, 'queued': True, 'tool_result': 'x' * 100_000}


def test_terminal_compaction_keeps_only_small_receipt_metadata(tmp_path, monkeypatch):
    monkeypatch.setattr(rq, '_RECENT_CALLS', 8)
    store = open_store(tmp_path)
    store.exec(1, 'prepare', CLOCK, intent())
    store.exec(1, 'append', CLOCK, {'kind': 'append', 'text': 'fixture-input', 'entry_id': 'entry'})
    store.exec(1, 'dispatch', CLOCK, {'kind': 'begin_dispatch', 'id': 'root', 'wire_id': 'terminal:1:root'})
    reply = {'operation_id': 'root', 'disposition': 'accepted', 'payload': native_payload()}
    first = store.exec(1, 'finish', CLOCK, {'kind': 'finish', 'id': 'root', 'status': 'accepted', 'result': reply})
    assert len(first['result']['payload']['tool_result']) == 100_000
    fill(store, 20)
    store = open_store(tmp_path)
    kept = store.state['operations']['root']
    assert kept['entry_materialized'] is True
    assert kept['result']['payload'] == {key: value for key, value in native_payload().items() if key != 'tool_result'}
    assert 'x' * 1000 not in store.state_path.read_text()
    assert len([key for key in store.state['operations'] if key.startswith('call::')]) == 8


@pytest.mark.parametrize('normalized', [False, True])
def test_terminal_finish_proof_matches_old_and_compact_receipts(tmp_path, normalized):
    store = open_store(tmp_path)
    operation = rq._operation('root', intent()['payload'], 'entry')
    result = {'operation_id': 'root', 'disposition': 'deferred', 'payload': {'cleanup': 'proved', 'code': 'input_unproved', 'bulk': 'x' * 1000}}
    operation.update(status='deferred', result=result, wire_attempts={'terminal:1:root': {'status': 'dispatching', 'result': None}})
    finish = {'kind': 'finish', 'id': 'root', 'status': 'deferred', 'result': result}
    if normalized:
        finish = rq._receipt_payload(finish)
    store.state.update(rows=[{'id': 'entry', 'text': 'fixture-input', 'ts': CLOCK['epoch_s'], 'delivered': True, 'attempts': 1}])
    store.state['operations'] = {'root': operation,
        'call::terminal:queue:2': rq._operation('call::terminal:queue:2', finish),
        'call::terminal:queue:3': rq._operation('call::terminal:queue:3', {'kind': 'bump_attempts', 'entry_id': 'entry'})}
    store.state['operations']['call::terminal:queue:3']['result'] = 1
    assert rq._terminal_finish_sequence(store.state, 'root', result) == 2
    store.exec(1, 'recover', CLOCK, {'kind': 'recover'})
    assert store.state['rows'][0]['attempts'] == 1
    assert store.state['rows'][0]['delivered'] is False
    assert store.state['operations']['root']['terminal_finalized'] is True


def legacy_state(case):
    state = rq.initial_state('key', 1, 'session', [])
    state.pop('next_seq'); state['version'] = 1
    def op(name, payload, entry=None, **values):
        item = rq._operation(name, payload, entry); item.pop('seq'); item.update(values)
        state['operations'][name] = item
        return item
    body = intent()['payload']
    if case != 'claim':
        root = op('root', body, 'entry')
        op('call::terminal:queue:1', intent(), status='accepted')
    if case != 'before_append':
        state['rows'] = [{'id': 'entry', 'text': 'fixture-input', 'ts': CLOCK['epoch_s'], 'delivered': True}]
    if case == 'removed':
        state['rows'] = []
        op('call::append', {'kind': 'append', 'text': 'fixture-input', 'entry_id': 'entry'},
           status='accepted', result={'id': 'entry'})
        op('call::remove', {'kind': 'remove', 'entry_id': 'entry'}, status='accepted', result=True)
    if case == 'claim':
        op('call::terminal:queue:1', {'kind': 'claim', 'entry_id': None, 'min_ts': 1, 'limit': 1},
           status='accepted', result=copy.deepcopy(state['rows']))
    if case == 'finish_bump':
        state['rows'][0]['attempts'] = 1
        result = {'operation_id': 'root', 'disposition': 'deferred', 'payload': {'cleanup': 'proved'}}
        root.update(status='deferred', result=result, wire_attempts={'terminal:1:root': {'status': 'unknown', 'result': None}})
        op('call::terminal:queue:2', {'kind': 'finish', 'id': 'root', 'status': 'deferred', 'result': result}, status='accepted')
        op('call::terminal:queue:3', {'kind': 'bump_attempts', 'entry_id': 'entry'}, status='accepted', result=1)
    return state


@pytest.mark.parametrize('case', ['before_append', 'removed', 'claim', 'finish_bump'])
def test_v1_terminal_proofs_survive_until_leased_recovery(tmp_path, case):
    path = tmp_path / 'key.queue-state.json'
    path.write_text(json.dumps(legacy_state(case)))
    from app.runtime_coordinator import WriterLease
    with closing(WriterLease(tmp_path / 'lease')):
        store = open_store(tmp_path)
        assert any(key.startswith('call::') for key in store.state['operations'])
        store.exec(1, 'recover', CLOCK, {'kind': 'recover'})
        assert store.state['version'] == 2
        if case == 'removed':
            assert store.state['rows'] == []
            assert store.state['operations']['root']['entry_materialized'] is True
        else:
            assert len(store.state['rows']) == 1 and store.state['rows'][0]['delivered'] is False
            if case == 'before_append':
                assert store.state['rows'][0]['text'] == 'fixture-input'
                assert store.state['operations']['root']['entry_materialized'] is True
            if case == 'finish_bump':
                assert store.state['rows'][0]['attempts'] == 1
                assert store.state['operations']['root']['terminal_finalized'] is True
        assert not any(key in store.state['operations'] for key in legacy_state(case)['operations'] if key.startswith('call::'))
        after = copy.deepcopy(store.state['rows'])
        store.exec(1, 'again', CLOCK, {'kind': 'recover'})
        assert store.state['rows'] == after


def test_legacy_prepare_receipt_backfills_terminal_generation_before_pruning(tmp_path):
    frozen = legacy_state('before_append')
    frozen['operations']['root']['payload']['payload'].pop('_terminal_generation')
    frozen['operations']['call::terminal:queue:1']['payload']['payload']['payload'].pop('_terminal_generation')
    (tmp_path / 'key.queue-state.json').write_text(json.dumps(frozen))
    from app.runtime_coordinator import WriterLease
    with closing(WriterLease(tmp_path / 'lease')):
        store = open_store(tmp_path)
        store.exec(1, 'recover', CLOCK, {'kind': 'recover'})
        assert 'call::terminal:queue:1' not in store.state['operations']
        assert store.state['operations']['root']['payload']['payload']['_terminal_generation'] == 1
        assert rq._terminal_input(store.state, store.state['operations']['root']) is True


def test_python_terminal_uses_durable_next_seq_after_call_pruning(monkeypatch, tmp_path):
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    state = copy.deepcopy(slot.store.state)
    state.update(next_seq=50_000, operations={})
    slot.store._persist(state)
    terminal._queue(owner, slot.binding.descriptor(), {'kind': 'set_runtime_state', 'state': {}})
    assert 'call::terminal:queue:50000' in slot.store.state['operations']


def test_python_terminal_prepare_final_has_zero_publication_or_key(monkeypatch, tmp_path):
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    state = copy.deepcopy(slot.store.state)
    state['rows'] = [{'id': 'root', 'text': 'fixture-input', 'ts': CLOCK['epoch_s'], 'delivered': True, 'confirmed': True}]
    state['operations'] = {}; state['next_seq'] = 50_000
    slot.store._persist(state)
    calls = []
    monkeypatch.setattr(terminal, '_service', lambda *args: calls.append('publication'))
    monkeypatch.setattr(ti.TerminalInput, 'send_prompt', lambda *args: calls.append('key'))
    async def flow():
        result = await owner.op('session', {'kind': 'submit', 'text': 'fixture-input'}, 'root')
        assert result['disposition'] == 'accepted'
        assert calls == []
    asyncio.run(flow())


def test_terminal_queue_io_leaves_slot_guard_available(monkeypatch, tmp_path):
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    atomic = slot.store._atomic
    def write(path, data):
        def check():
            available = slot.guard.acquire(blocking=False)
            if available:
                slot.guard.release()
            return available
        with ThreadPoolExecutor(max_workers=1) as pool:
            assert pool.submit(check).result(1)
        atomic(path, data)
    monkeypatch.setattr(slot.store, '_atomic', write)
    terminal._queue(owner, slot.binding.descriptor(), {'kind': 'set_runtime_state', 'state': {}})


@pytest.mark.parametrize('attempt', ['staged', 'dispatching'])
def test_python_recover_requeues_only_attempts_that_never_started_writing(tmp_path, attempt):
    """O boot recupera a fila no Python antes do Rust: a regra do `staged` vale dos dois lados."""
    store = open_store(tmp_path)
    store.exec(1, 'prepare', CLOCK, intent())
    store.exec(1, 'append', CLOCK, {'kind': 'append', 'text': 'fixture-input', 'entry_id': 'entry'})
    store.exec(1, 'dispatch', CLOCK, {'kind': 'begin_dispatch', 'id': 'root', 'wire_id': 'terminal:1:root'})
    state = copy.deepcopy(store.state)
    state['operations']['root']['wire_attempts']['terminal:1:root']['status'] = attempt   # como o Rust grava
    store._persist(state)
    store = open_store(tmp_path)
    store.exec(1, 'recover', CLOCK, {'kind': 'recover'})
    op, row = store.state['operations']['root'], store.state['rows'][0]
    if attempt == 'staged':
        assert op['status'] == 'deferred' and op['result']['payload']['code'] == 'interrupted_before_write'
        assert row['delivered'] is False and 'terminal_write_barrier' not in store.state['runtime_state']
    else:
        assert op['status'] == 'unknown' and store.state['runtime_state'].get('terminal_write_barrier')
