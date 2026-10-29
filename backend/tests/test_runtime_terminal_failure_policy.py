import asyncio
import json

import pytest

from app import runtime_adapter as ra, runtime_coordinator as rc, runtime_queue as rq, runtime_terminal as terminal
from app.runtime_coordinator import Phase, WriterLease
from app.rust_server import RustOpError
from test_runtime_terminal import live_owner


class Gateway:
    instance = 'terminal-policy-test'
    alive = True

    def __init__(self, code, failures):
        self.code, self.failures = code, failures
        self.leases, self.attempts, self.events_log = {}, [], []
        self.drain_attempts = []

    def snapshot(self, target, revision=0, error=None):
        return {'key': target['key'], 'generation': target['generation'], 'revision': revision,
                'view': {'terminal': True, 'conversation': target['meta']['session_id'],
                         'deliverable': error is None, 'preserve_binding': None, 'clear_barrier': None},
                'channels': {}, 'error': error}

    async def op(self, target, command, operation_id, clock):
        if command['kind'] == 'ingress':
            return {'closed': command['closed']}
        name, kind = target['name'], command['kind']
        if kind == 'open':
            self.leases[name] = WriterLease(target['lock_path'])
            return {'opened': True, 'instance': self.instance, 'key': target['key'],
                    'generation': target['generation'], 'state': self.snapshot(target)}
        if kind == 'close':
            self.events_log.append(('close', name))
            self.leases.pop(name).close()
            return {'closed': True}
        if kind == 'submit' and name == 'session':
            self.attempts.append(operation_id)
            self.events_log.append(('attempt', len(self.attempts)))
            if len(self.attempts) <= self.failures:
                if self.code in {'queue_io', 'body_unknown'}:
                    store = rq.QueueStore(target['state_path'], target['projection_dir'],
                        rq.initial_state(target['key'], target['generation'], name, []))
                    body = {'kind': 'input', 'payload': {'text': command['text'],
                            '_terminal_generation': target['generation']}}
                    for index, action in enumerate([
                        {'kind': 'prepare', 'id': operation_id, 'payload': body, 'entry_id': operation_id},
                        {'kind': 'append', 'text': command['text'], 'entry_id': operation_id,
                         'delivered': False, 'ts': clock['epoch_s'], 'pre_transcript': False},
                        {'kind': 'begin_dispatch', 'id': operation_id, 'wire_id': 'terminal:1:root'},
                    ]):
                        store.exec(target['generation'], f'fake-dispatch:{index}', clock, action)
                    if self.code == 'body_unknown':
                        result = {'operation_id': operation_id, 'disposition': 'unknown',
                                  'payload': {'stage': 'write', 'code': 'mux_effect_failed'}}
                        store.exec(target['generation'], 'fake-finish', clock,
                            {'kind': 'finish', 'id': operation_id, 'status': 'unknown', 'result': result})
                        return result
                raise RustOpError(f'IPC recusou a operação (503: {self.code})', 503, self.code)
        if kind == 'snapshot':
            return self.snapshot(target, 1, self.code if self.code == 'terminal_facts' else None)
        if kind == 'drain' and self.code == 'terminal_facts':
            self.drain_attempts.append(operation_id)
            raise RustOpError('IPC recusou a operação (503: terminal_facts)', 503, self.code)
        return {'operation_id': operation_id, 'disposition': 'accepted', 'payload': {}}

    def close(self):
        for lease in self.leases.values():
            lease.close()
        self.leases.clear()


async def _to_rust(owner, name):
    """Registro Python do terminal aberto no Rust (o caminho do vínculo pendente que provou a conversa)."""
    await owner._open_slot_in_rust(name, owner.slot(name), launch=False)
    return True


@pytest.fixture(autouse=True)
def isolated_owner(monkeypatch):
    monkeypatch.setattr(rc, '_current', None)
    monkeypatch.setattr(rq, '_coordinator', None)


def setup(monkeypatch, tmp_path, gateway):
    from app import diag, terminal_input
    owner, slot, collected = live_owner(monkeypatch, tmp_path, gateway=gateway)
    monkeypatch.setattr(terminal, '_collect', lambda name: {
        **collected, 'name': name, 'pane': '%1' if name == 'session' else '%2',
        'session_id': 'sid' if name == 'session' else 'other-sid',
        'jsonl': str(tmp_path / ('sid.jsonl' if name == 'session' else 'other-sid.jsonl')),
    })
    effects, records = [], []
    monkeypatch.setattr(terminal_input.TerminalInput, 'send_prompt',
        lambda self, name, text: effects.append((name, text)) or 'sent')
    monkeypatch.setattr(diag, 'registrar', lambda event, level='info', **fields:
        records.append((event, level, fields)))
    original_sleep = asyncio.sleep

    async def pause(delay):
        gateway.events_log.append(('pause', delay))
        await original_sleep(0)
    monkeypatch.setattr(rc.asyncio, 'sleep', pause)
    return owner, slot, effects, records


def test_terminal_rust_refusal_raises_once_and_session_stays_rust(monkeypatch, tmp_path):
    gateway = Gateway('runtime_lease', 99)
    owner, slot, effects, records = setup(monkeypatch, tmp_path, gateway)

    async def flow():
        other = owner.register(terminal.resolve_binding('other'))
        assert await _to_rust(owner,'session') and await _to_rust(owner,'other')
        with pytest.raises(RustOpError) as caught:
            await owner.op('session', {'kind': 'submit', 'text': 'private-test-message'}, 'root')
        assert caught.value.code == 'runtime_lease'
        assert gateway.attempts == ['root'] and effects == []
        assert slot.phase == Phase.Rust and other.phase == Phase.Rust
        failures_logged = [fields for event, _, fields in records if event == 'runtime.rust_op_failed']
        assert len(failures_logged) == 1 and failures_logged[0]['codigo'] == 'runtime_lease'
        assert 'private-test-message' not in json.dumps(records)
        assert (await owner.op('other', {'kind': 'submit', 'text': 'other-input'}, 'other'))['disposition'] == 'accepted'
        assert effects == []

    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


def test_terminal_possible_effect_is_not_repeated_and_row_stays_protected(monkeypatch, tmp_path):
    gateway = Gateway('queue_io', 99)
    owner, slot, effects, records = setup(monkeypatch, tmp_path, gateway)

    async def flow():
        assert await _to_rust(owner,'session')
        with pytest.raises(RustOpError):
            await owner.op('session', {'kind': 'submit', 'text': 'private-test-message'}, 'root')
        assert gateway.attempts == ['root'] and effects == []
        assert slot.phase == Phase.Rust
        state = json.loads(slot.binding.state_path.read_bytes())
        assert state['operations']['root']['status'] == 'dispatching'
        assert state['rows'][0]['delivered'] is True
        assert 'private-test-message' not in json.dumps(records)

    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


def test_terminal_normal_refusal_does_not_retry_or_change_owner(monkeypatch, tmp_path):
    gateway = Gateway('claude_command', 99)
    owner, slot, effects, records = setup(monkeypatch, tmp_path, gateway)

    async def flow():
        assert await _to_rust(owner,'session')
        with pytest.raises(RustOpError):
            await owner.op('session', {'kind': 'submit', 'text': 'stale-control'}, 'root')
        assert gateway.attempts == ['root'] and effects == []
        assert slot.phase == Phase.Rust
        assert not any(event == 'runtime.rust_op_failed' for event, _, _ in records)
    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


def test_terminal_maintenance_error_still_hands_input_to_rust(monkeypatch, tmp_path):
    # Recado que passa pelo Python (grupo, par, MCP) com o terminal em erro de manutenção: o Rust
    # guarda a linha na fila durável; recusar aqui perdia o recado.
    gateway = Gateway('terminal_facts', 0)
    owner, slot, effects, records = setup(monkeypatch, tmp_path, gateway)

    async def flow():
        assert await _to_rust(owner,'session')
        await owner.refresh_snapshot('session')
        assert slot.view['error'] == 'terminal_facts' and not slot.cache_valid
        result = await owner.op('session', {'kind': 'submit', 'text': 'peer-message'}, 'root')
        assert result['disposition'] == 'accepted'
        assert gateway.attempts == ['root'] and slot.phase == Phase.Rust
        assert 'close' not in [kind for kind, _ in gateway.events_log]
    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


def test_terminal_private_snapshot_is_accepted_without_public_state(monkeypatch, tmp_path):
    gateway = Gateway('runtime_lease', 0)
    owner, slot, _, _ = setup(monkeypatch, tmp_path, gateway)
    async def flow():
        assert await _to_rust(owner,'session')
        data = gateway.snapshot(slot.binding.descriptor(), 1)
        event = {'key': slot.binding.key, 'generation': 1, 'revision': 1, 'channel': 'snapshot', 'data': data}
        assert ra.apply_event(slot, event)
        assert slot.cache_valid and ra.native_slot('session') is None
        assert 'public_state' not in slot.view['view'] and slot.view['channels'] == {}
        data['view']['conversation'] = 'another-conversation'
        assert not ra.apply_event(slot, event)
        assert not ra.apply_event(slot, {**event, 'revision': 2, 'channel': 'state', 'data': {}})
    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


class UnknownDelivery(Gateway):
    """A entrega fica incerta: o ator entra em `terminal_delivery_unknown` até reabrir."""

    def __init__(self):
        super().__init__('body_unknown', 1)
        self.kinds, self.revision, self.error = [], 1, None

    async def op(self, target, command, operation_id, clock):
        self.kinds.append(command['kind'])
        if command['kind'] == 'snapshot':
            self.revision += 1
            return self.snapshot(target, self.revision, self.error)
        if command['kind'] == 'open':
            self.error = None
        result = await super().op(target, command, operation_id, clock)
        if command['kind'] == 'submit' and result.get('disposition') == 'unknown':
            self.error = 'terminal_delivery_unknown'
        return result


def test_terminal_unknown_delivery_reopens_in_rust(monkeypatch, tmp_path):
    gateway = UnknownDelivery()
    owner, slot, effects, records = setup(monkeypatch, tmp_path, gateway)
    async def flow():
        assert await _to_rust(owner,'session')
        result = await owner.op('session', {'kind': 'submit', 'text': 'private-test-message'}, 'root')
        assert result['disposition'] == 'unknown'
        assert slot.phase == Phase.Rust, 'entrega incerta não passa a sessão ao Python'
        # O Rust publica o problema; a próxima operação reabre a sessão no Rust, com ator novo.
        assert ra.apply_event(slot, {'key': slot.binding.key, 'generation': 1, 'revision': slot.view['revision'] + 1,
                                     'channel': 'problem', 'data': {'error_code': 'terminal_delivery_unknown'}})
        assert ra.runtime_problem('session') == ('runtime_falhou', 'terminal_delivery_unknown')
        gateway.kinds.clear()
        result = await owner.op('session', {'kind': 'submit', 'text': 'next-input'}, 'next')
        assert result['disposition'] == 'accepted'
        # A reabertura congela a sessão: a porta do Rust fecha antes do `close` e reabre depois do `open`.
        assert gateway.kinds == ['snapshot', 'ingress', 'close', 'open', 'ingress', 'submit']
        assert slot.phase == Phase.Rust and effects == []
        assert ra.runtime_problem('session') is None
        assert any(event == 'runtime.reopened' for event, _, _ in records)
        assert 'private-test-message' not in json.dumps(records)
    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


def test_terminal_error_snapshot_triggers_shared_maintenance_without_device(monkeypatch, tmp_path):
    original_sleep = asyncio.sleep
    gateway = Gateway('terminal_facts', 99)
    owner, slot, effects, records = setup(monkeypatch, tmp_path, gateway)
    finished = None
    async def events():
        data = gateway.snapshot(slot.binding.descriptor(), 1, 'terminal_facts')
        yield {'key': slot.binding.key, 'generation': 1, 'revision': 1, 'channel': 'snapshot', 'data': data}
        await finished.wait()
    gateway.events = events
    async def flow():
        nonlocal finished
        finished = asyncio.Event()
        owner.loop = asyncio.get_running_loop()
        assert await _to_rust(owner,'session')
        task = asyncio.create_task(owner._events(gateway, gateway.instance))
        try:
            for _ in range(100):
                if gateway.drain_attempts:
                    break
                await original_sleep(.01)
            await original_sleep(.05)
            # A manutenção do erro de fatos é do Rust: falha sobe ao diário, a sessão fica nele.
            assert gateway.drain_attempts and slot.phase == Phase.Rust
            assert effects == [] and 'close' not in [kind for kind, _ in gateway.events_log]
            assert any(event == 'runtime.drain_failed' for event, _, _ in records)
        finally:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


def test_terminal_stalled_input_surfaces_reason_once_per_series(monkeypatch, tmp_path):
    gateway = Gateway('runtime_lease', 0)
    owner, slot, _, records = setup(monkeypatch, tmp_path, gateway)
    async def flow():
        assert await _to_rust(owner, 'session')
        assert ra.runtime_problem('session') is None
        for revision, stalled in enumerate(['composer_busy', 'composer_busy', 'capture_utf8', 'ui_novo', None], 1):
            data = gateway.snapshot(slot.binding.descriptor(), revision)
            data['view']['input_stalled'] = stalled
            assert ra.apply_event(slot, {'key': slot.binding.key, 'generation': 1, 'revision': revision,
                                          'channel': 'snapshot', 'data': data})
            expected = {'composer_busy': 'terminal_input_composer_busy', 'capture_utf8': 'terminal_input_capture_failed',
                        'ui_novo': 'terminal_input_stalled'}.get(stalled)
            assert ra.runtime_problem('session') == ((expected, stalled) if stalled else None)
        stalls = [fields['codigo'] for event, _, fields in records if event == 'terminal.input_stalled']
        assert stalls == ['composer_busy', 'capture_utf8', 'ui_novo']
        bad = gateway.snapshot(slot.binding.descriptor(), 9)
        bad['view']['input_stalled'] = 7
        assert not ra.apply_event(slot, {'key': slot.binding.key, 'generation': 1, 'revision': 9, 'channel': 'snapshot', 'data': bad})
    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


class RustAdvancedLife(Gateway):
    """O Rust troca a conversa sozinho (`/clear`) e passa à geração 2: só aceita pedidos dela."""

    def __init__(self):
        super().__init__('runtime_lease', 0)
        self.generation, self.closed_with = 1, []

    async def op(self, target, command, operation_id, clock):
        if command['kind'] == 'close':
            self.closed_with.append(target['generation'])
            if target['generation'] != self.generation:
                raise RustOpError('IPC recusou a operação (503: runtime_generation)', 503, 'runtime_generation')
        return await super().op(target, command, operation_id, clock)


def test_python_follows_the_generation_the_rust_advanced_and_close_works(monkeypatch, tmp_path):
    # Parity A1: `/clear` (ou reabertura em bypass) avança a geração no Rust; o Python seguia com a
    # velha e fechar, renomear e trocar de conta recebiam `runtime_generation`.
    gateway = RustAdvancedLife()
    owner, slot, _, _ = setup(monkeypatch, tmp_path, gateway)
    finished = None
    async def events():
        # O Rust grava a vida nova na fila antes de avisar.
        state = json.loads(slot.binding.state_path.read_bytes())
        slot.binding.state_path.write_text(json.dumps({**state, 'generation': 2}))
        gateway.generation = 2
        yield {'key': slot.binding.key, 'generation': 2, 'revision': 0, 'channel': 'reset',
               'data': {'conversation': 'sid2', 'generation': 2}}
        await finished.wait()
    gateway.events = events
    async def flow():
        nonlocal finished
        finished = asyncio.Event()
        owner.loop = asyncio.get_running_loop()
        assert await _to_rust(owner, 'session')
        task = asyncio.create_task(owner._events(gateway, gateway.instance))
        try:
            for _ in range(50):
                if slot.binding.generation == 2:
                    break
                await asyncio.sleep(0)
            assert slot.binding.generation == 2
            assert slot.binding.meta['session_id'] == 'sid2' and slot.binding.meta['terminal']['conversation'] == 'sid2'
            assert slot.binding.meta['terminal']['generation'] == 2
            assert slot.binding.jsonl.endswith('sid2.jsonl')
            reply = await owner.detach('session', restore=False)
            assert reply == {'closed': True} and gateway.closed_with == [2]
        finally:
            finished.set()
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
    try:
        asyncio.run(flow())
    finally:
        gateway.close()
        owner.close_python_leases()


def test_claude_headless_opens_managed_with_the_sidecar_dir():
    # Parity A3: sem a pasta o Rust abria o Claude sem terminal sem administrar o processo, e o que caía
    # ficava parado até alguém sair e entrar na conversa.
    from app.adapters.claude_headless import sessions
    headless = {'provider': 'claude', 'headless': True, 'name': 's'}
    assert rc._open_command(headless) == {'kind': 'open', 'descriptor': {**headless, 'sidecar_dir': str(sessions._dir())}}
    terminal = {'provider': 'claude', 'headless': False, 'name': 's'}
    assert rc._open_command(terminal) == {'kind': 'open', 'descriptor': terminal}
