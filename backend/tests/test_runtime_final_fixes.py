import copy
import json
import asyncio
from datetime import datetime, timezone

from app import runtime_queue as rq
from app.runtime_receipt import ReceiptIndex
from test_runtime_queue import CLOCK, open_store, fill
from test_runtime_terminal import isolated_owner, live_owner
import pytest


def test_absent_transcript_echo_confirms_only_one_equal_input_after_compact_reopen(tmp_path, monkeypatch):
    monkeypatch.setattr(rq, '_RECENT_CALLS', 8)
    store = open_store(tmp_path)
    transcript = tmp_path / 'absent.jsonl'
    index = ReceiptIndex('claude', 'sid')
    cursor = index.capture(transcript)
    assert cursor['file_identity'] is None
    for number in (1, 2):
        operation = f'input-{number}'
        for kind, action in [
            ('append', {'kind': 'append', 'text': 'same-input', 'entry_id': operation}),
            ('prepare', {'kind': 'prepare', 'id': operation, 'entry_id': operation, 'payload': {'kind': 'input'}}),
            ('cursor', {'kind': 'bind_dispatch', 'id': operation, 'cursor': cursor}),
            ('dispatch', {'kind': 'begin_dispatch', 'id': operation, 'wire_id': operation}),
        ]:
            store.exec(1, f'{kind}:{number}', CLOCK, action)
    timestamp = datetime.fromtimestamp(cursor['absent_since'] + 1, timezone.utc).isoformat()
    transcript.write_text(json.dumps({'type': 'user', 'uuid': 'one-echo', 'sessionId': 'sid',
        'timestamp': timestamp, 'message': {'content': 'same-input'}}) + '\n')
    index.scan(transcript)
    proof = index.match_after(cursor, store.state['rows'][0], store.state['used_occurrences'])
    assert proof is not None
    assert store.exec(1, 'confirm-first', CLOCK, {'kind': 'confirm_occurrence', 'id': 'input-1', 'proof': proof}) is True
    fill(store, 20)
    store = open_store(tmp_path)
    assert list(store.state['used_occurrences']) == [proof['occurrence']['id']]
    assert index.match_after(cursor, store.state['rows'][1], store.state['used_occurrences']) is None
    duplicate = copy.deepcopy(proof)
    assert store.exec(1, 'confirm-second', CLOCK, {'kind': 'confirm_occurrence', 'id': 'input-2', 'proof': duplicate}) is False
    assert store.state['rows'][0]['confirmed'] is True
    assert not store.state['rows'][1].get('confirmed')


def test_startup_unresolved_same_terminal_life_recovers_without_second_restart(tmp_path, monkeypatch):
    from app import runtime_coordinator as rc, runtime_terminal as terminal, pqueue
    from app.adapters.claude_headless import sessions as claude_sessions
    from app.adapters.codex import sessions as codex_sessions
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    slot.store.exec(1, 'append', CLOCK, {'kind': 'append', 'text': 'pending-input', 'entry_id': 'pending'})
    original = copy.deepcopy(slot.store.state['rows'])
    owner.close_python_leases()
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    monkeypatch.setattr(claude_sessions, 'list_all', lambda: [])
    monkeypatch.setattr(codex_sessions, 'list_all', lambda: [])
    identity = {'value': None}
    monkeypatch.setattr(terminal, '_collect', lambda name: identity['value'])
    restored = rc.RuntimeCoordinator()
    async def flow():
        await restored.start_sessions({'claude': object(), 'codex': object()})
        waiting = restored.slot('session')
        assert waiting.phase == rc.Phase.RecoveringPython and waiting.store is None and waiting.lease is None
        identity['value'] = collected
        assert await restored.prepare_session('session', 'claude') is True
        current = restored.slot('session')
        assert current.phase == rc.Phase.Python and current.lease is not None and not current.lease.closed
        assert current.binding.key == slot.binding.key and current.binding.generation == slot.binding.generation
        assert current.store.state['rows'] == original
        current.lease.close()
    asyncio.run(flow())


@pytest.mark.parametrize('plugin_live', [False, True])
@pytest.mark.parametrize('transition', ['restart', 'same_sid_generation'])
def test_unknown_fill_blocks_next_entry_and_submit_across_transition(tmp_path, monkeypatch, plugin_live, transition):
    from app import runtime_terminal as terminal, terminal_input as ti
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    screen, effects = {'text': ''}, []
    phase = {'first': True}
    real_service = terminal._service
    def service(coordinator, descriptor, operation, request, kind, payload):
        if kind == 'terminal_facts':
            return dict(binding=payload['binding'], ready=True, idle=True, open_question=False,
                plugin_live=phase['first'] or plugin_live, plugin_user=False, native=None, clipboard_available=False)
        if kind == 'terminal_publish':
            effects.append('publication')
            screen['text'] = payload['publication']['text']
            return 'unknown'
        return real_service(coordinator, descriptor, operation, request, kind, payload)
    monkeypatch.setattr(terminal, '_service', service)
    monkeypatch.setattr(terminal, '_python_prompt', lambda *args: effects.extend(['cleanup', 'text', 'Enter']) or ('sent', False, ''))
    monkeypatch.setattr(ti, '_capture', lambda name: '─'*30+'\n❯ '+screen['text']+'\n'+'─'*30+'\n')
    async def flow():
        nonlocal owner, slot
        assert (await owner.op('session', {'kind': 'submit', 'text': 'A'}, 'A'))['disposition'] == 'unknown'
        slot.store.exec(1, 'append-B', CLOCK, {'kind': 'append', 'text': 'B', 'entry_id': 'B'})
        phase['first'] = False
        if transition == 'restart':
            owner.close_python_leases()
            owner, slot, _ = live_owner(monkeypatch, tmp_path)
        else:
            async def noop():
                return None
            await owner.change('session', noop, advance=True, reopen=False)
            assert slot.binding.generation == 2 and slot.binding.meta['terminal']['conversation'] == 'sid'
        assert (await owner.op('session', {'kind': 'drain'}, 'drain-B'))['sent'] == 0
        result = await owner.op('session', {'kind': 'submit', 'text': 'C'}, 'C')
        assert result['disposition'] == 'deferred'
        assert effects == ['publication'] and screen['text'] == 'A'
        assert slot.store.state['operations']['A']['status'] == 'unknown'
        assert not next(row for row in slot.store.state['rows'] if row['id'] == 'B')['delivered']
        assert not next(row for row in slot.store.state['rows'] if row['id'] == 'C')['delivered']
    asyncio.run(flow())


def test_unknown_composer_allows_control_and_proven_new_conversation_without_retyping(tmp_path, monkeypatch):
    from app import runtime_terminal as terminal, terminal_input as ti
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    phase, effects = {'first': True}, []
    real_service = terminal._service
    def service(coordinator, descriptor, operation, request, kind, payload):
        if kind == 'terminal_facts':
            return dict(binding=payload['binding'], ready=True, idle=True, open_question=False,
                plugin_live=phase['first'], plugin_user=False, native=None, clipboard_available=False)
        if kind == 'terminal_publish':
            effects.append('A-publication'); return 'unknown'
        return real_service(coordinator, descriptor, operation, request, kind, payload)
    monkeypatch.setattr(terminal, '_service', service)
    monkeypatch.setattr(ti.TerminalInput, 'interrupt', lambda *args, **kwargs: effects.append('control'))
    monkeypatch.setattr(terminal, '_python_prompt', lambda *args: effects.append('new-conversation-input') or ('sent', False, ''))
    async def flow():
        assert (await owner.op('session', {'kind': 'submit', 'text': 'A'}, 'A'))['disposition'] == 'unknown'
        phase['first'] = False
        assert (await owner.op('session', {'kind': 'control', 'control': 'interrupt', 'payload': {'clear': False}}, 'control'))['disposition'] == 'accepted'
        assert (await owner.op('session', {'kind': 'submit', 'text': 'B'}, 'B'))['disposition'] == 'deferred'
        assert effects == ['A-publication', 'control']
        collected.update(session_id='new-sid', jsonl=str(tmp_path / 'new-sid.jsonl'))
        assert await owner.prepare_session('session', 'claude')
        assert (await owner.op('session', {'kind': 'submit', 'text': 'C'}, 'C'))['disposition'] == 'accepted'
        assert effects == ['A-publication', 'control', 'new-conversation-input']
    asyncio.run(flow())


def _uncertain_terminal_inputs(tmp_path, texts):
    store = open_store(tmp_path)
    transcript = tmp_path / 'sid.jsonl'
    index = ReceiptIndex('claude', 'sid')
    cursor = index.capture(transcript)
    for text in texts:
        for kind, action in [
            ('append', {'kind': 'append', 'text': text, 'entry_id': text}),
            ('prepare', {'kind': 'prepare', 'id': text, 'entry_id': text,
                         'payload': {'kind': 'input', 'payload': {'text': text, '_terminal_generation': 1}}}),
            ('cursor', {'kind': 'bind_dispatch', 'id': text, 'cursor': cursor}),
            ('dispatch', {'kind': 'begin_dispatch', 'id': text, 'wire_id': f'terminal:1:{text}'}),
            ('finish', {'kind': 'finish', 'id': text, 'status': 'unknown',
                        'result': {'operation_id': text, 'disposition': 'unknown', 'payload': {'cleanup': 'uncertain'}}}),
        ]:
            store.exec(1, f'{kind}:{text}', CLOCK, action)
    timestamp = datetime.fromtimestamp(cursor['absent_since'] + 1, timezone.utc).isoformat()
    transcript.write_text(''.join(json.dumps({'type': 'user', 'uuid': f'echo-{text}', 'sessionId': 'sid',
        'timestamp': timestamp, 'message': {'content': text}}) + '\n' for text in texts))
    index.scan(transcript)
    def confirm(text):
        row = next(row for row in store.state['rows'] if row['id'] == text)
        proof = index.match_after(cursor, row, store.state['used_occurrences'])
        return store.exec(1, f'confirm:{text}', CLOCK, {'kind': 'confirm_occurrence', 'id': text, 'proof': proof})
    return store, confirm


@pytest.mark.parametrize('resolution', ['confirm', 'late_accepted'])
def test_terminal_write_barrier_follows_remaining_uncertain_input_and_lifts_after_last(tmp_path, resolution):
    store, confirm = _uncertain_terminal_inputs(tmp_path, ['A', 'B'])
    assert store.state['runtime_state']['terminal_write_barrier']['operation_id'] == 'A'
    assert rq.terminal_write_blocked(store.state, 'sid')
    if resolution == 'confirm':
        assert confirm('A') is True
    else:
        store.exec(1, 'late:A', CLOCK, {'kind': 'finish', 'id': 'A', 'status': 'accepted',
            'result': {'operation_id': 'A', 'disposition': 'accepted', 'payload': {}}})
    # B continua incerta na mesma conversa: a trava passa para ela.
    assert store.state['runtime_state']['terminal_write_barrier']['operation_id'] == 'B'
    assert rq.terminal_write_blocked(store.state, 'sid')
    assert confirm('B') is True
    assert 'terminal_write_barrier' not in store.state['runtime_state']
    assert not rq.terminal_write_blocked(store.state, 'sid')
    reopened = open_store(tmp_path)
    assert not rq.terminal_write_blocked(reopened.state, 'sid')


def test_confirmed_uncertain_input_releases_drain_and_next_submit(tmp_path, monkeypatch):
    from app import runtime_terminal as terminal, terminal_input as ti
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    phase, effects = {'first': True}, []
    real_service = terminal._service
    def service(coordinator, descriptor, operation, request, kind, payload):
        if kind == 'terminal_facts':
            return dict(binding=payload['binding'], ready=True, idle=True, open_question=False,
                plugin_live=phase['first'], plugin_user=False, native=None, clipboard_available=False)
        if kind == 'terminal_publish':
            effects.append('A-publication'); return 'unknown'
        return real_service(coordinator, descriptor, operation, request, kind, payload)
    monkeypatch.setattr(terminal, '_service', service)
    monkeypatch.setattr(terminal, '_python_prompt', lambda *args: effects.append('typed') or ('sent', False, ''))
    async def flow():
        assert (await owner.op('session', {'kind': 'submit', 'text': 'A'}, 'A'))['disposition'] == 'unknown'
        phase['first'] = False
        slot.store.exec(1, 'append-B', CLOCK, {'kind': 'append', 'text': 'B', 'entry_id': 'B'})
        assert (await owner.op('session', {'kind': 'drain'}, 'drain-blocked'))['sent'] == 0
        operation = slot.store.state['operations']['A']
        transcript = tmp_path / 'sid.jsonl'
        cursor = operation['dispatch_cursor']
        stamp = cursor.get('absent_since') or 0
        transcript.write_text(json.dumps({'type': 'user', 'uuid': 'echo-A', 'sessionId': 'sid',
            'timestamp': datetime.fromtimestamp(stamp + 1, timezone.utc).isoformat(),
            'message': {'content': 'A'}}) + '\n')
        await owner.op('session', {'kind': 'confirm'}, 'confirm-A')
        assert slot.store.state['operations']['A']['status'] == 'confirmed'
        assert (await owner.op('session', {'kind': 'drain'}, 'drain-free'))['sent'] == 1
        assert effects == ['A-publication', 'typed']
    asyncio.run(flow())


def test_awaiting_identity_record_retires_when_binding_returns_with_other_key(tmp_path, monkeypatch):
    from app import runtime_coordinator as rc, runtime_terminal as terminal, pqueue
    from app.adapters.claude_headless import sessions as claude_sessions
    from app.adapters.codex import sessions as codex_sessions
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    old_key = slot.binding.key
    owner.close_python_leases()
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    monkeypatch.setattr(claude_sessions, 'list_all', lambda: [])
    monkeypatch.setattr(codex_sessions, 'list_all', lambda: [])
    identity = {'value': None}
    monkeypatch.setattr(terminal, '_collect', lambda name: identity['value'])
    restored = rc.RuntimeCoordinator()
    async def flow():
        await restored.start_sessions({'claude': object(), 'codex': object()})
        waiting = restored.slot('session')
        assert waiting.awaiting_identity and waiting.lease is None and waiting.store is None
        identity['value'] = {**collected, 'namespace': 'socket:other-pid:other-birth'}
        for _ in range(3):
            assert await restored.prepare_session('session', 'claude') is True
        current = restored.slot('session')
        assert current.binding.key != old_key
        assert current.phase == rc.Phase.Python and current.lease is not None and not current.lease.closed
        assert waiting.phase == rc.Phase.RecoveringPython and waiting.lease is None
        current.lease.close()
    asyncio.run(flow())


@pytest.mark.parametrize('text', ['/clear', '  /clear keep'])
def test_clear_with_arguments_passes_terminal_write_barrier_like_rust(tmp_path, monkeypatch, text):
    from app import runtime_terminal as terminal
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    phase, effects = {'first': True}, []
    real_service = terminal._service
    def service(coordinator, descriptor, operation, request, kind, payload):
        if kind == 'terminal_facts':
            return dict(binding=payload['binding'], ready=True, idle=True, open_question=False,
                plugin_live=phase['first'], plugin_user=False, native=None, clipboard_available=False)
        if kind == 'terminal_publish':
            effects.append('A-publication'); return 'unknown'
        return real_service(coordinator, descriptor, operation, request, kind, payload)
    monkeypatch.setattr(terminal, '_service', service)
    monkeypatch.setattr(terminal, '_python_prompt', lambda *args: effects.append('typed') or ('sent', False, ''))
    async def flow():
        assert (await owner.op('session', {'kind': 'submit', 'text': 'A'}, 'A'))['disposition'] == 'unknown'
        phase['first'] = False
        result = await owner.op('session', {'kind': 'submit', 'text': text}, 'clear')
        assert (result.get('payload') or {}).get('code') != 'terminal_write_barrier'
        assert effects == ['A-publication', 'typed']
    asyncio.run(flow())


def test_uncertain_input_of_a_second_conversation_blocks_writes_while_old_barrier_stays(tmp_path):
    store, _ = _uncertain_terminal_inputs(tmp_path, ['A'])
    cursor = ReceiptIndex('claude', 'new-sid').capture(tmp_path / 'new-sid.jsonl')
    for kind, action in [
        ('append', {'kind': 'append', 'text': 'C', 'entry_id': 'C'}),
        ('prepare', {'kind': 'prepare', 'id': 'C', 'entry_id': 'C',
                     'payload': {'kind': 'input', 'payload': {'text': 'C', '_terminal_generation': 2}}}),
        ('cursor', {'kind': 'bind_dispatch', 'id': 'C', 'cursor': cursor}),
        ('dispatch', {'kind': 'begin_dispatch', 'id': 'C', 'wire_id': 'terminal:2:C'}),
        ('finish', {'kind': 'finish', 'id': 'C', 'status': 'unknown',
                    'result': {'operation_id': 'C', 'disposition': 'unknown', 'payload': {'cleanup': 'uncertain'}}}),
    ]:
        store.exec(1, f'{kind}:C', CLOCK, action)
    assert store.state['runtime_state']['terminal_write_barrier']['conversation'] == 'sid'
    assert rq.terminal_write_blocked(store.state, 'new-sid')
    store.exec(1, 'late:C', CLOCK, {'kind': 'finish', 'id': 'C', 'status': 'accepted',
        'result': {'operation_id': 'C', 'disposition': 'accepted', 'payload': {}}})
    assert not rq.terminal_write_blocked(store.state, 'new-sid')
    assert rq.terminal_write_blocked(store.state, 'sid')


def test_killing_a_terminal_session_does_not_revalidate_the_dead_binding(tmp_path, monkeypatch):
    from app import runtime_terminal as terminal
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    killed = []
    async def kill():
        # O pane morre com o kill: daqui em diante não há vínculo para resolver.
        monkeypatch.setattr(terminal, '_collect', lambda name: None)
        killed.append(True)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        await owner.change('session', kill, remove=True)
    asyncio.run(flow())
    assert killed and 'session' not in owner.names and slot.binding.key not in owner.slots


def test_waiting_record_of_a_dead_life_never_holds_the_name(tmp_path, monkeypatch):
    from app import runtime_coordinator as rc, runtime_terminal as terminal, pqueue
    from app.adapters.claude_headless import sessions as claude_sessions
    from app.adapters.codex import sessions as codex_sessions
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    owner.close_python_leases()
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    monkeypatch.setattr(claude_sessions, 'list_all', lambda: [])
    monkeypatch.setattr(codex_sessions, 'list_all', lambda: [])
    monkeypatch.setattr(terminal, '_collect', lambda name: None)
    restored = rc.RuntimeCoordinator()
    monkeypatch.setattr(rc, '_current', restored)
    async def flow():
        restored.loop = asyncio.get_running_loop()
        await restored.start_sessions({'claude': object(), 'codex': object()})
        assert restored.slot('session').awaiting_identity
        closed = []
        async def kill():
            closed.append(True)
        await restored.change('session', kill, remove=True)
        assert closed
        assert not restored.managed_queue('session')
    asyncio.run(flow())


@pytest.mark.parametrize('born, claims', [(2.0, False), (1.0, True)])
def test_waiting_record_only_claims_the_name_in_its_own_tmux_life(tmp_path, monkeypatch, born, claims):
    # Outra vida tmux com o mesmo nome (aqui nascida em 2; o registro guarda 1) é de outra sessão,
    # até de outro provedor: o registro morto não pode reservar o nome dela.
    from app import diag, runtime_coordinator as rc, runtime_terminal as terminal, pqueue, tmux
    from app.adapters.claude_headless import sessions as claude_sessions
    from app.adapters.codex import sessions as codex_sessions
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    owner.close_python_leases()
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    monkeypatch.setattr(claude_sessions, 'list_all', lambda: [])
    monkeypatch.setattr(codex_sessions, 'list_all', lambda: [])
    monkeypatch.setattr(terminal, '_collect', lambda name: None)
    monkeypatch.setattr(tmux, 'sessao_existe', lambda name: True)
    monkeypatch.setattr(tmux, 'session_created', lambda name: born)
    events = []
    monkeypatch.setattr(diag, 'registrar', lambda evento, *a, **k: events.append(evento))
    restored = rc.RuntimeCoordinator()
    monkeypatch.setattr(rc, '_current', restored)
    async def flow():
        restored.loop = asyncio.get_running_loop()
        await restored.start_sessions({'claude': object(), 'codex': object()})
    asyncio.run(flow())
    assert restored.managed_queue('session') is claims
    assert ('runtime.registration_failed' in events) is claims
    assert ('runtime.stale_terminal_record' in events) is not claims


def test_one_session_failing_to_recover_does_not_stop_the_takeover(monkeypatch):
    from types import SimpleNamespace
    from app import diag, runtime_coordinator as rc
    recovered, logged = [], []
    coordinator = rc.RuntimeCoordinator()
    coordinator.slots = {k: SimpleNamespace(binding=SimpleNamespace(name=k, key=k), phase=rc.Phase.Rust, awaiting_identity=False)
                         for k in ('a', 'b')}
    coordinator.slots['c'] = SimpleNamespace(binding=SimpleNamespace(name='c', key='c'), phase=rc.Phase.RecoveringPython,
                                             awaiting_identity=True)
    coordinator.names = {'a': 'a', 'b': 'b', 'c': 'c'}
    async def recover(name, confirmed_dead, containment=None):
        if name in ('a', 'c'):
            raise RuntimeError('vida não volta')
        recovered.append(name)
    coordinator.recover = recover
    monkeypatch.setattr(diag, 'registrar', lambda evento, nivel, **kw: logged.append((evento, kw.get('sessao'))))
    asyncio.run(coordinator.enter_python())
    assert recovered == ['b'] and ('runtime.recover_failed', 'a') in logged
    assert ('runtime.recover_failed', 'c') not in logged and coordinator.mode == "python"


def test_failed_recovery_retires_the_record_so_the_name_is_not_stuck(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from app import runtime_coordinator as rc, runtime_terminal as terminal
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    slot.phase = rc.Phase.Rust
    monkeypatch.setattr(terminal, '_collect', lambda name: None)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        with pytest.raises(RuntimeError):
            await owner.recover('session', confirmed_dead=True, containment=SimpleNamespace(containment_clean=lambda: True))
        assert not owner.managed_queue('session') and slot.binding.key not in owner.slots
        assert slot.lease is None
        # A vida volta: o nome se registra de novo, com o estado durável.
        monkeypatch.setattr(terminal, '_collect', lambda name: {**collected, 'name': name})
        assert await owner.prepare_session('session', 'claude') is True
        assert owner.slot('session').phase == rc.Phase.Python
        owner.slot('session').lease.close()
    asyncio.run(flow())


def test_kill_after_the_binding_changed_before_closing_still_kills(tmp_path, monkeypatch):
    from app import runtime_terminal as terminal
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    # /clear feito direto na TUI antes do fechar: a conversa mudou.
    monkeypatch.setattr(terminal, '_collect', lambda name: {**collected, 'name': name,
        'jsonl': str(tmp_path / 'other.jsonl'), 'session_id': 'other'})
    killed = []
    async def kill():
        killed.append(True)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        await owner.change('session', kill, remove=True)
    asyncio.run(flow())
    assert killed and not owner.managed_queue('session')


def test_select_respects_open_panel(monkeypatch):
    from app import api, runtime_terminal
    checked, routed = [], []
    monkeypatch.setattr(api, '_cached_info_sync', lambda name: None)
    monkeypatch.setattr(api, '_recusa_se_painel_aberto', lambda name: checked.append(name))
    monkeypatch.setattr(runtime_terminal, 'route_sync', lambda name, command: routed.append(command) or {'ok': True})
    assert api.select('s', api.SelectBody(option=1)) == {'ok': True}
    assert checked == ['s']
    assert routed[0]['payload'] == {'option': 1}


@pytest.mark.parametrize('error,status,code', [
    (RuntimeError('vínculo gerenciado indisponível; escrita suspensa'), 503, 'erro_opcao_nao_convergiu'),
    # A tecla pode ter chegado: nunca "NÃO enviada", que convida a repetir.
    ('outcome_unknown', 409, 'erro_sem_confirmacao_resposta'),
    ('control_error', None, None), ('transfer', None, None)])
def test_select_terminal_route_failure_is_a_coded_error_not_a_bare_500(monkeypatch, error, status, code):
    from fastapi import HTTPException
    from app import api, runtime_terminal
    from app.runtime_coordinator import TransferInProgress
    error = {'outcome_unknown': runtime_terminal.TerminalOutcomeUnknown('resultado terminal incerto'),
             'control_error': runtime_terminal.TerminalControlError('select', 'deferred', None),
             'transfer': TransferInProgress('posse em troca')}.get(error, error)
    monkeypatch.setattr(api, '_cached_info_sync', lambda name: None)
    monkeypatch.setattr(api, '_recusa_se_painel_aberto', lambda name: None)
    def failing(name, command):
        raise error
    monkeypatch.setattr(runtime_terminal, 'route_sync', failing)
    if status is None:      # têm handler próprio no app (409): sobem como vieram
        with pytest.raises(type(error)):
            api.select('s', api.SelectBody(option=1))
        return
    with pytest.raises(HTTPException) as caught:
        api.select('s', api.SelectBody(option=1))
    assert caught.value.status_code == status
    assert caught.value.detail['code'] == code


def test_queue_fsync_never_runs_on_the_event_loop(tmp_path, monkeypatch):
    # Com o disco ocupado o fsync leva segundos: no laço de eventos ele parava o backend inteiro.
    import threading
    from types import SimpleNamespace
    from app import runtime_coordinator as rc, runtime_queue, runtime_terminal as terminal
    owner, slot, collected = live_owner(monkeypatch, tmp_path)
    slot.phase = rc.Phase.Rust
    monkeypatch.setattr(terminal, '_collect', lambda name: None)
    on_loop, loop_thread = [], threading.get_ident()      # asyncio.run roda o laço nesta thread
    atomic = runtime_queue.QueueStore._atomic
    def watched(path, data):
        on_loop.append(threading.get_ident() == loop_thread)
        return atomic(path, data)
    monkeypatch.setattr(runtime_queue.QueueStore, '_atomic', staticmethod(watched))
    async def flow():
        owner.loop = asyncio.get_running_loop()
        with pytest.raises(RuntimeError):
            await owner.recover('session', confirmed_dead=True, containment=SimpleNamespace(containment_clean=lambda: True))
        monkeypatch.setattr(terminal, '_collect', lambda name: {**collected, 'name': name})
        assert await owner.prepare_session('session', 'claude') is True     # registro novo: _restore
        assert await owner.prepare_session('session', 'claude') is True     # vínculo gravado de novo
        owner.slot('session').lease.close()
    asyncio.run(flow())
    assert on_loop and not any(on_loop)


def test_select_on_headless_codex_answers_the_approval_without_the_claude_terminal_route(monkeypatch):
    # Codex sem terminal não tem vínculo Claude: a rota do terminal suspendia a escrita e o cartão
    # de aprovação não se respondia (500).
    from types import SimpleNamespace
    from app import api, runtime_coordinator as rc, runtime_terminal
    owner = rc.RuntimeCoordinator()
    owner.mode = 'python'
    owner.legacy = SimpleNamespace(binding=lambda *args: None)
    monkeypatch.setattr(rc, '_current', owner, raising=False)
    monkeypatch.setattr(rc, 'current', lambda: owner)
    monkeypatch.setattr(runtime_terminal, 'being_born', lambda name, after=0: False)
    monkeypatch.setattr(runtime_terminal, 'outside_scope', lambda name: False)
    monkeypatch.setattr(api, '_recusa_se_painel_aberto', lambda name: None)
    monkeypatch.setattr(api, '_session_exists', lambda name: True)
    monkeypatch.setattr(api, '_headless', lambda name: False)
    monkeypatch.setattr(api, '_cached_info_sync', lambda name: SimpleNamespace(provider='codex', headless=True))
    answered = []
    async def select(name, option):
        answered.append((name, option))
        return True
    monkeypatch.setattr(api, 'get_adapter', lambda provider: SimpleNamespace(select=select))
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        monkeypatch.setattr(api, '_loop_servidor', owner.loop)
        return await asyncio.to_thread(api.select, 'cx', api.SelectBody(option=1))
    assert asyncio.run(scenario()) == {'ok': True}
    assert answered == [('cx', 1)]
