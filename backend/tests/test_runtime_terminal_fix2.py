import asyncio
from types import SimpleNamespace

import pytest

from app import api, pqueue, procinfo, registry, runtime_coordinator as rc, runtime_terminal as terminal, terminal_input as ti, tmux
from test_runtime_terminal import isolated_owner, live_owner


def current_processes(monkeypatch, commands, *, hidden=False):
    monkeypatch.setattr(tmux, 'list_panes_all', lambda: {
        'session': [{'pid': 100, 'pane_id': '%1', 'provider': 'pi', 'hidden': hidden}]})
    argv = {100: ['sh'], **{101 + i: command for i, command in enumerate(commands)}}
    children = {100: list(reversed(list(argv)[1:]))}
    ages = []
    def fresh_map(max_age=999):
        ages.append(max_age)
        return children
    monkeypatch.setattr(procinfo, '_proc_children_map', fresh_map)
    monkeypatch.setattr(procinfo, '_argv', lambda pid: argv.get(pid, []))
    monkeypatch.setattr(registry, '_argv', lambda pid: argv.get(pid, []))
    monkeypatch.setattr(registry, '_cmdline', lambda pid: ' '.join(argv.get(pid, [])))
    return ages


@pytest.mark.parametrize('commands', [
    [['claude', '--session-id', 'sid']],
    [['pi'], ['claude', '--session-id', 'sid']],
    [],
])
def test_stale_provider_or_ambiguous_process_never_allows_unbound_writer(monkeypatch, tmp_path, commands):
    ages = current_processes(monkeypatch, commands)
    owner = rc.RuntimeCoordinator()
    owner.legacy = SimpleNamespace(binding=lambda *args: None)
    monkeypatch.setattr(rc, '_current', owner)
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    projection = tmp_path / 'session.jsonl'
    projection.write_text('{"text":"preserved"}\n')
    before = projection.read_bytes()
    monkeypatch.setattr(api, '_pane_info', lambda name: ('claude', '%1'))
    monkeypatch.setattr(api, '_session_exists', lambda name: True)
    monkeypatch.setattr(api, '_headless', lambda name: False)
    monkeypatch.setattr(api, '_provider_of', lambda name: 'claude')
    effects = []
    monkeypatch.setattr(api, '_enviar_nativo', lambda *args: effects.append('socket') or 'mid')
    monkeypatch.setattr(api.terminal, 'send_prompt', lambda *args, **kwargs: effects.append('key') or 'sent')
    monkeypatch.setattr(api, '_agendar_confirmacao', lambda *args: None)
    async def scenario():
        owner.loop = asyncio.get_running_loop()
        result = await asyncio.to_thread(api._send_one, 'session', 'text')
        assert result['ok'] is False
        assert effects == []
        assert owner.names == {}
        assert projection.read_bytes() == before
        with pytest.raises(RuntimeError, match='sem vínculo comprovado'):
            terminal.assert_writer('session')
        assert ages and all(age == 0 for age in ages)
    asyncio.run(scenario())


@pytest.mark.parametrize('commands,hidden', [
    ([['pi']], False), ([['omp']], False), ([['kimi']], False), ([['codex']], False),
    ([['claude', 'auth']], False), ([], True), ([['pi'], ['pi']], False),
    ([['claude', '--session-id', 'sid', '--plugin-dir', '/p', 'auth', 'login', '--claudeai']], False),
    ([['claude', '--session-id=sid', '--plugin-dir=/p', 'setup-token']], False),
])
def test_proven_outside_scope_preserves_existing_drivers(monkeypatch, commands, hidden):
    ages = current_processes(monkeypatch, commands, hidden=hidden)
    owner = rc.RuntimeCoordinator()
    owner.legacy = SimpleNamespace(binding=lambda *args: None)
    monkeypatch.setattr(rc, '_current', owner)
    assert terminal.outside_scope('session') is True
    assert asyncio.run(owner.prepare_session('session', 'claude')) is False
    terminal.assert_writer('session')
    assert ages and all(age == 0 for age in ages)


def test_wrapper_session_without_auth_stays_in_scope(monkeypatch):
    current_processes(monkeypatch, [['claude', '--session-id', 'sid', '--plugin-dir', '/p']])
    assert terminal.outside_scope('session') is False


@pytest.mark.parametrize('commands,allowed', [
    ([['claude', '--session-id', 'sid', '--plugin-dir', '/p', 'auth', 'login', '--claudeai']], True),
    ([['claude', '--session-id', 'sid']], False),
])
def test_dead_life_record_does_not_block_login_window(monkeypatch, commands, allowed):
    current_processes(monkeypatch, commands, hidden=True)
    owner = rc.RuntimeCoordinator()
    owner.legacy = SimpleNamespace(binding=lambda *args: None)
    owner.names, owner.slots = {'session': 'k'}, {'k': rc.Slot(binding=SimpleNamespace(meta={'terminal': {'name': 'session'}}),
                                                              awaiting_identity=True)}
    monkeypatch.setattr(rc, '_current', owner)
    if allowed:
        terminal.assert_writer('session')
    else:
        with pytest.raises(RuntimeError, match='posse da escrita'):
            terminal.assert_writer('session')


@pytest.mark.parametrize('text,content,accepted', [
    ('mensagem comprida\na', 'rascunho alheio', False),
    ('sim', 'sim', True),
    ('mensagem comprida\na', '[Pasted text #2 +3 lines]', True),
    ('mensagem comprida\na', 'mensagem comprida\na', True),
])
def test_filled_reserve_requires_sufficient_matching_text(monkeypatch, tmp_path, text, content, accepted):
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    def pane(value):
        return '─' * 30 + '\n❯ ' + value + '\n' + '─' * 30 + '\n'
    screen = {'value': pane('[Pasted text #1 +3 lines]')}
    real_service = terminal._service
    def service(coordinator, descriptor, operation, request, kind, payload):
        if kind == 'terminal_facts':
            return dict(binding=payload['binding'], ready=True, idle=True, open_question=False,
                plugin_live=True, plugin_user=False, native=None, clipboard_available=False)
        if kind == 'terminal_publish':
            screen['value'] = pane(content)
            return 'filled'
        return real_service(coordinator, descriptor, operation, request, kind, payload)
    monkeypatch.setattr(terminal, '_service', service)
    monkeypatch.setattr(ti, '_capture', lambda name: screen['value'])
    effects = []
    def enter(*args, **kwargs):
        effects.append('Enter')
        screen['value'] = pane('')
        return True
    monkeypatch.setattr(tmux, 'send_keys', enter)
    async def scenario():
        result = await owner.op('session', {'kind': 'submit', 'text': text}, 'filled')
        assert result['disposition'] == ('accepted' if accepted else 'unknown')
        assert effects == (['Enter'] if accepted else [])
    asyncio.run(scenario())
