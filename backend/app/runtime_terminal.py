"""Vínculo terminal, serviços sem teclado e reserva sob a posse do coordenador."""
from __future__ import annotations

import asyncio
import contextvars
import copy
import hashlib
import json
import logging
import os
import socket
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

from app.runtime_coordinator import Binding, Phase, WriterLease, _clock

_log = logging.getLogger("hangar.runtime_terminal")

_writer = contextvars.ContextVar('runtime_terminal_writer', default=None)
# Pedido de convidado (clique de mod pelo app): não recebe o teclado de uma sessão do Rust. A marca
# viaja com o contexto até a thread do driver, que a passa adiante ao `run_admin`.
guest_admin = contextvars.ContextVar('runtime_terminal_guest_admin', default=False)


# Teto do teclado emprestado pelo Rust a uma administração (troca de modelo/motor, /btw, modo).
_LOAN_S = 60
# Folga sobre o prazo do Rust: o Python para de digitar antes de o Rust retomar o pane.
_LOAN_MARGIN_S = 1


class KeyboardLoanExpired(RuntimeError):
    """O prazo do teclado emprestado venceu: o Rust retomou o pane e a operação do Python falha."""
    code = 'keyboard_loan_expired'
    safe_detail = True


class TerminalOutcomeUnknown(RuntimeError):
    """A entrada pode ter chegado ao pane: quem chama não repete nem diz que nada foi enviado."""


class TerminalControlError(RuntimeError):
    def __init__(self, control, disposition, code):
        self.control, self.disposition, self.code = control, disposition, code
        super().__init__('O controle não foi executado; confira a sessão.' if disposition == 'deferred'
            else 'Não foi possível confirmar o controle; confira a sessão antes de repetir.')


class GuestRefused(Exception):
    """Convidado pediu o teclado de uma sessão cujo terminal é do Rust. Não é `RuntimeError` de
    propósito: quem trata falha de escrita como "sem resposta" não pode engolir a recusa."""


def outside_scope(name):
    from app import tmux, registry as registry_mod, procinfo
    panes = tmux.list_panes_all().get(name)
    if not panes:
        return False
    children = procinfo._proc_children_map(max_age=0)
    agents = {}
    unresolved = False
    for pane in panes:
        current = {}
        if pane['pid']:
            for pid in procinfo._descendant_pids(pane['pid'], children):
                provider, agent = registry_mod.agente_do_pane(pid, {})
                if agent is not None:
                    current[agent] = provider
        agents.update(current)
        unresolved |= not current and not pane.get('hidden')
    if unresolved:
        return False
    if not agents:
        return all(pane.get('hidden') for pane in panes)
    return all(provider != 'claude' or _claude_login(pid) for pid, provider in agents.items())


# Opções que o wrapper do Hangar põe antes dos argumentos de quem chamou.
_WRAPPER_OPTIONS = ('--session-id', '--plugin-dir')


def _claude_login(pid):
    from app import procinfo
    arguments = procinfo._argv(pid)[1:]
    while arguments and arguments[0].startswith(_WRAPPER_OPTIONS):
        arguments = arguments[1:] if '=' in arguments[0] else arguments[2:]
    return bool(arguments) and arguments[0] in {'auth', 'login', 'setup-token'}


# Do pane ao agente com a conversa provada leva poucos segundos; além disso não é nascimento.
BIRTH_WINDOW_S = 15


def being_born(name, after=0):
    """Pane criado há pouco, de uma vida não anterior a `after`, cujo agente ainda não provou a conversa."""
    from app import tmux
    panes = tmux.list_panes_all().get(name)
    born = max((pane.get('session_created') or 0 for pane in panes or ()), default=0)
    return bool(born) and born >= after and time.time() - born < BIRTH_WINDOW_S and not outside_scope(name)


def _collect(name):
    from app import api, claude_customizations, tmux, registry as registry_mod, procinfo
    import psutil
    panes = tmux.list_panes_all().get(name)
    if not panes:
        return None
    children = procinfo._proc_children_map(max_age=0)
    agents = [registry_mod.agente_do_pane(candidate['pid'], children)[1] for candidate in panes]
    if len([agent for agent in agents if agent is not None]) != 1:
        return None
    pane = registry_mod.SessionRegistry._agent_pane(panes, children)
    provider, agent = registry_mod.agente_do_pane(pane['pid'], children)
    if provider != 'claude' or agent is None or _claude_login(agent):
        return None
    if os.name == 'nt':
        matching = [item for item in tmux.list_panes_of(name) if item.get('pid') == pane['pid']
            and item.get('pane_id') == pane['pane_id']]
        if len(matching) != 1:
            return None
        pane = {**pane, **matching[0]}
    target = tmux.alvo_de_pane(name, pane)
    if not target:
        return None
    format_string = '#{socket_path}\t#{pid}\t#{session_created}\t#{pane_id}\t#{session_name}\t#{session_id}'
    cp = tmux._run(['tmux', 'display-message', '-p', '-t', target, format_string])
    if cp.returncode:
        return None
    fields = cp.stdout.strip().split('\t')
    if len(fields) != 6 or not fields[0] or not fields[1].isdigit() or not fields[2].isdigit() or fields[4] != name:
        return None
    mux_pid, created = int(fields[1]), int(fields[2])
    namespace = f'{fields[0]}:{mux_pid}:{psutil.Process(mux_pid).create_time()}'
    pane_birth = psutil.Process(pane['pid']).create_time()
    config_dir = str(procinfo._config_dir_of(agent) or api.registry.projects_dir.parent)
    jsonl, tracked = api.registry.resolve_tracked(name, pane['cwd'], pid=pane['pid'], children=children)
    if not jsonl or not tracked:
        return None
    # Um cache de nome não é prova de uma vida nova. Exige processo e vínculo atuais.
    cmd = procinfo._cmdline(agent)
    sid = registry_mod._session_id_from_cmdline(cmd)
    agent_birth = psutil.Process(agent).create_time()
    record = Path(config_dir) / 'sessions' / f'{agent}.json'
    if record.exists():
        native = json.loads(record.read_bytes())
        if int(native.get('pid') or 0) != agent:
            raise RuntimeError('registro terminal de outro processo')
        current_sid = native.get('sessionId')
        if current_sid:
            from app.registry import sanitize_cwd
            jsonl = str(Path(config_dir) / 'projects' / sanitize_cwd(pane['cwd']) / f'{current_sid}.jsonl')
            sid = current_sid
    if not sid:
        marker = registry_mod._marker_by_pids(Path(config_dir), [agent], set())
        opened = procinfo._open_jsonl(agent, Path(config_dir) / 'projects')
        if jsonl not in {marker, opened}:
            return None
    elif Path(jsonl).stem != sid:
        marker = registry_mod._active_marker_jsonl(Path(config_dir), sid, set())
        opened = procinfo._open_jsonl(agent, Path(config_dir) / 'projects')
        if jsonl not in {marker, opened}:
            return None
    after = tmux._run(['tmux', 'display-message', '-p', '-t', target, format_string])
    if (after.returncode or after.stdout.strip().split('\t') != fields
            or psutil.Process(pane['pid']).create_time() != pane_birth
            or f'{fields[0]}:{mux_pid}:{psutil.Process(mux_pid).create_time()}' != namespace):
        raise RuntimeError('vida terminal mudou durante a resolução')
    return dict(name=name, pane=target, created=created, namespace=namespace,
        pane_birth=pane_birth, agent_pid=agent, agent_birth=agent_birth,
        session_proof=_session_hash(namespace, fields[5], created),
        jsonl=jsonl, session_id=Path(jsonl).stem, config_dir=config_dir, cwd=pane['cwd'],
        mux_argv=['tmux'], windows=os.name == 'nt',
        claude_settings=claude_customizations.from_environment(
            procinfo._env_var_of(agent, claude_customizations.SESSION_SETTINGS_ENV)),
        plugin_token=procinfo._env_var_of(agent, 'HANGAR_PLUGIN_TOKEN') or '')


def resolve_binding(name, previous=None):
    from app.pqueue import _queue_dir
    import psutil
    try:
        facts = _collect(name)
    except psutil.Error:
        return None     # pane, agente ou servidor tmux saiu no meio da leitura: não há vida a provar
    if facts is None:
        return None
    fingerprint = hashlib.sha256(json.dumps([facts['namespace'], facts['pane'].split(':')[-1],
        facts['created'], facts.get('pane_birth')], ensure_ascii=False).encode()).hexdigest()
    same = previous is not None and (previous.meta.get('fingerprint') == fingerprint
        or previous.meta.get('pending_terminal') and previous.meta['pending_terminal'] == facts.get('session_proof')
            and previous.meta.get('session_id') in (None, facts['session_id'])
        or previous.headless and previous.meta.get('session_id') == facts['session_id'])
    key = previous.key if same else 'terminal_' + fingerprint
    plugin_key = facts.get('plugin_token', '').partition('.')[0] if '.' in facts.get('plugin_token', '') else None
    kept = previous.meta.get('plugin_key') if same and previous.meta.get('agent_pid') == facts.get('agent_pid') else None
    if plugin_key is None and kept:
        # O ambiente do mesmo processo não muda: chave que sumiu é leitura que falhou, não token sem chave.
        _log.warning('chave da ponte não lida do ambiente; mantida a anterior sessao=%s', name)
        plugin_key = kept
    directory = _queue_dir()
    state_path = previous.state_path if same else directory / 'runtime' / f'{key}.json'
    generation = previous.generation if same else json.loads(state_path.read_bytes())['generation'] if state_path.exists() else 1
    terminal = dict(name=name, pane=facts['pane'], conversation=facts['session_id'], generation=generation,
        created=facts['created'], mux_argv=facts['mux_argv'], windows=facts['windows'],
        clipboard_lock_path=str(directory / 'runtime' / 'clipboard.lock'))
    return Binding(name, key, 'claude', False, dict(key=key, headless=False, terminal=terminal,
        fingerprint=fingerprint, session_id=facts['session_id'], config_dir=facts['config_dir'],
        cwd=facts['cwd'], created=previous.meta.get('created', 0) if same else max(facts['created'], facts.get('pane_birth', facts['created'])),
        legacy_import_after=facts.get('pane_birth'), agent_pid=facts.get('agent_pid'),
        agent_birth=facts.get('agent_birth'),
        **({'claude_settings': facts['claude_settings']} if facts.get('claude_settings') is not None else {}),
        # A chave do token que o processo recebeu no lançamento: a ponte do Rust acha a sessão por ela.
        **({'plugin_key': plugin_key} if plugin_key else {})), facts['jsonl'],
        previous.projection_dir if same else directory, state_path,
        previous.lock_path if same else directory / 'runtime' / f'{key}.lock', generation)


class BindingChanged(RuntimeError):
    # A mensagem leva só nomes de campos: pode ir ao diário.
    safe_detail = True


def validate_binding(descriptor):
    values = copy.deepcopy(descriptor)
    for field in ('projection_dir', 'state_path', 'lock_path'):
        values[field] = Path(values[field])
    previous = Binding(**values)
    current = resolve_binding(previous.name, previous)
    if current is None:
        raise BindingChanged('vínculo terminal mudou (sem vida atual); nenhuma escrita autorizada')
    differs = [field for field, now, before in (('key', current.key, previous.key), ('jsonl', current.jsonl, previous.jsonl),
        *((field, current.meta.get(field), previous.meta.get(field)) for field in ('fingerprint', 'agent_pid', 'agent_birth')),
        *(('terminal.' + field, current.meta['terminal'].get(field), previous.meta['terminal'].get(field))
          for field in sorted(set(current.meta['terminal']) | set(previous.meta['terminal'])))) if now != before]
    if differs:
        raise BindingChanged(f"vínculo terminal mudou ({','.join(differs)}); nenhuma escrita autorizada")
    return current


async def quiesce(coordinator, descriptor):
    await coordinator._wait_active(coordinator.slots[descriptor['key']])
    def wait_legacy():
        from app.terminal_input import _send_lock
        with _send_lock(descriptor['name']):
            if descriptor.get('removed'):
                return
            if descriptor['meta'].get('pending_terminal'):
                if _session_proof(descriptor['name']) != descriptor['meta']['pending_terminal']:
                    raise RuntimeError('vida terminal pendente mudou')
            else:
                validate_binding(descriptor)
    await asyncio.to_thread(wait_legacy)
    return {'terminal':True}


async def reconnect(coordinator, descriptor, carry):
    try:
        await asyncio.to_thread(validate_binding, descriptor)
    except RuntimeError:
        values = copy.deepcopy(descriptor)
        for field in ('projection_dir', 'state_path', 'lock_path'):
            values[field] = Path(values[field])
        replacement = await asyncio.to_thread(resolve_binding, descriptor['name'], Binding(**values))
        if (replacement is None or replacement.key != descriptor['key']
                or not coordinator.in_lifecycle(coordinator.slots[descriptor['key']])):
            raise
    return {'hydrated':True}


@contextmanager
def writer(coordinator, descriptor):
    token = _writer.set((coordinator, copy.deepcopy(descriptor)))
    try:
        assert_writer(descriptor['name'])
        yield
    finally:
        _writer.reset(token)


@contextmanager
def borrowed(coordinator, descriptor, deadline):
    """Escrita do Python no pane de uma sessão do Rust, durante o teclado emprestado."""
    token = _writer.set((coordinator, copy.deepcopy(descriptor), deadline))
    try:
        assert_writer(descriptor['name'])
        yield
    finally:
        _writer.reset(token)


def assert_writer(name):
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is None:
        return
    if not coordinator.managed_queue(name):
        if coordinator.legacy is not None and not outside_scope(name):
            raise RuntimeError('Claude terminal sem vínculo comprovado; escrita suspensa')
        return
    slot = coordinator.slot(name)
    # Registro de uma vida morta só reserva o nome; a janela nova com ele pode ser só um login.
    if slot.awaiting_identity and outside_scope(name):
        return
    if slot.binding.meta.get('pending_terminal'):
        raise RuntimeError('vínculo terminal ainda não confirmado; escrita suspensa')
    if not slot.binding.meta.get('terminal'):
        from app.runtime_adapter import assert_legacy
        assert_legacy(name)
        return
    authorization = _writer.get()
    if slot.phase == Phase.Rust:
        # Sessão do Rust: só escreve quem tem o teclado emprestado, e só dentro do prazo.
        if (authorization is None or len(authorization) != 3 or authorization[0] is not coordinator
                or authorization[1]['key'] != slot.binding.key or authorization[1]['generation'] != slot.binding.generation):
            raise RuntimeError('Python sem posse da escrita terminal')
        if time.monotonic() >= authorization[2]:
            raise KeyboardLoanExpired('prazo do teclado emprestado venceu; o Rust retomou o pane')
        validate_binding(authorization[1])
        return
    if (authorization is None or len(authorization) != 2 or authorization[0] is not coordinator
            or authorization[1]['key'] != slot.binding.key
            or authorization[1]['generation'] != slot.binding.generation
            or slot.phase != Phase.Python or slot.lease is None or slot.lease.closed
            or slot.frozen and not coordinator.in_lifecycle(slot)):
        raise RuntimeError('Python sem posse da escrita terminal')
    validate_binding(authorization[1])


def facts(payload, metadata):
    from app import terminal_input as ti, plugin_bridge as plugin, askquestion, uds_messaging, api, tmux
    metadata['validate']()
    descriptor = metadata['descriptor']
    current = validate_binding(descriptor)
    binding = payload.get('binding')
    if binding != current.meta['terminal'] or payload.get('operation_id') != metadata['operation_id']:
        raise ValueError('fatos de outra operação ou vínculo')
    text = payload.get('text')
    if not isinstance(text, str):
        raise ValueError('fatos sem texto')
    screen = ti._capture(current.name)
    ready = ti._composer_regiao(screen, current.name) is not None
    state, _, _, _ = ti.classify(screen)
    ready = ready and state != 'awaiting_input' and not ti.is_overlay(screen)
    idle = state == 'idle'
    native_state = _native_state(current)
    if native_state is not None:
        idle = native_state == 'idle'
    else:
        plugin_state = plugin.estado_recente(current.name) if _plugin_current(current) else None
        hook_state = api.hook_state.get_state(current.meta['session_id'])
        if plugin_state is not None:
            idle = plugin_state[0] == 'idle'
        elif hook_state is not None and hook_state[1] >= current.meta.get('legacy_import_after', current.meta.get('created', 0)):
            idle = hook_state[0] == 'idle'
    question = bool(askquestion.pergunta_aberta(Path(current.jsonl).stem) or plugin.pergunta_pendente(current.name))
    modes = plugin.declared_modes(current.name)
    live = plugin.aguardando(current.name) and _plugin_current(current)
    native = None
    sender, _ = uds_messaging.separar_prefixo(text)
    endpoint = _native_socket(current) if sender and os.name == 'posix' else None
    if endpoint:
        native = dict(socket=endpoint, origin=f'uds:{uds_messaging.INBOX.path}' if uds_messaging.INBOX.path else f'uds:hangar-{os.getpid()}',
            sender=sender, mode=api._classe_modo(sender, current.name, current.meta['config_dir'], current.jsonl),
            message_id=str(uuid.uuid5(uuid.NAMESPACE_URL, f"hangar:{current.key}:{payload['operation_id']}")))
    clipboard = False
    if binding['windows']:
        ours, theirs = tmux._sessao_windows_de(os.getpid()), tmux._sessao_windows_de(tmux.pane_pid(current.name))
        clipboard = ours is not None and theirs is not None and ours > 0 and ours == theirs
    metadata['validate']()
    return dict(binding=binding, ready=ready, idle=idle, open_question=question,
        plugin_live=live, plugin_user=live and 'user' in modes, clipboard_available=clipboard, native=native)


def publish(payload, metadata):
    from app import plugin_bridge
    metadata['validate']()
    current = validate_binding(metadata['descriptor'])
    if payload.get('binding') != current.meta['terminal'] or payload.get('generation') != current.generation:
        raise ValueError('publicação de outra vida')
    if not _plugin_current(current):
        return 'not_written'
    return plugin_bridge.publish_terminal(current.name, current.meta['session_id'], current.generation,
        payload['publication'], metadata['validate'])


def plugin_control(payload, metadata):
    from app import plugin_bridge
    from app.adapters.claude_headless.adapter import respostas_do_app, _mensagem_conversar
    metadata['validate']()
    current = validate_binding(metadata['descriptor'])
    if payload.get('binding') != current.meta['terminal'] or payload.get('generation') != current.generation:
        raise ValueError('controle de outra vida')
    body = payload['payload']
    request_id = body.get('request_id')
    pending = plugin_bridge.pergunta_pendente(current.name)
    if not pending:
        return {'disposition':'rejected'}
    normalized_id = pending['id'] if str(pending['id']).startswith(('perm:', 'ask:')) else 'ask:' + pending['id']
    if normalized_id != request_id:
        return {'disposition':'rejected'}
    # Pergunta `ask:` corre também no diálogo da TUI: sem resposta pelo plugin, escolhe o teclado.
    # Permissão segurada pelo plugin não tem menu no terminal; lá a recusa é a resposta.
    fallback = {'disposition':'rejected' if request_id.startswith('perm:') else 'unavailable'}
    if payload['control'] == 'select' and not request_id.startswith('perm:'):
        return fallback
    if 'receipt_v2' not in plugin_bridge.declared_modes(current.name):
        return fallback
    if not _plugin_current(current):
        return fallback
    if payload['control'] == 'select' and request_id.startswith('perm:'):
        if body.get('option') not in (1,2):
            raise ValueError('opção fora do pedido de permissão')
        response = {'permitir':body['option'] == 1}
    elif payload['control'] == 'answer_questions' and pending['questions']:
        answers, conversation = respostas_do_app(pending['questions'], body['answers'])
        response = {'deny':_mensagem_conversar(answers, conversation)} if conversation else {'answers':answers}
    else:
        return {'disposition':'rejected'}
    metadata['validate']()
    receipt = {'publication_id':payload['publication_id'], 'generation':current.generation,
        'session_id':current.meta['session_id']}
    if plugin_bridge.responder_pergunta(current.name, response, pending['id'], receipt):
        return {'disposition':'accepted'}
    return {'disposition':'unknown'}


async def _return_lost_loan(coordinator, name, descriptor, request, request_id):
    """A concessão pode ter saído e a resposta se perdido: o mesmo pedido repetido traz a mesma
    concessão (o Rust deduplica pelo id), que volta na hora em vez de prender o teclado até o prazo.
    Vai direto ao canal, sem a espera da vista que a perda de transporte acabou de invalidar."""
    from app import diag
    from app.runtime_coordinator import failure_reason
    try:
        again = await coordinator._rpc(descriptor, request, request_id)
    except Exception as exc:
        # Sem a concessão em mãos o teclado pode ficar com o Python até o prazo (`_LOAN_S`).
        diag.registrar('runtime.keyboard_loan_unconfirmed', 'erro', sessao=name, **failure_reason(exc))
        return
    if again.get('disposition') != 'accepted':
        return          # nada concedido a este pedido: não há o que devolver
    try:
        await coordinator._rpc(descriptor, {'kind':'control', 'control':'keyboard_return',
            'payload':{'loan_id':again['payload']['loan_id']}}, 'admin:' + uuid.uuid4().hex)
    except Exception as exc:
        diag.registrar('runtime.keyboard_return_failed', 'erro', sessao=name, **failure_reason(exc))


async def _borrow_keyboard(coordinator, name, action, *, guest=False):
    """Administração que digita no pane de uma sessão do Rust: ele pausa as próprias escritas e
    empresta o teclado por uma operação, com prazo; fila, trava e estado continuam com ele.
    `guest`: pedido de convidado, recusado aqui, sob a barreira, com a posse já conferida."""
    from app import diag
    from app.runtime_coordinator import failure_reason
    slot = coordinator.slot(name)
    async with coordinator._barrier(slot):      # fechar/renomear espera; envios seguem para a fila
        if coordinator.slots.get(coordinator.names.get(name, '')) is not slot or slot.phase != Phase.Rust:
            raise RuntimeError('a sessão mudou de dono antes da administração; tente de novo')
        if guest:
            raise GuestRefused('convidado não recebe o teclado de uma sessão do Rust')
        descriptor = slot.binding.descriptor()
        await asyncio.to_thread(validate_binding, descriptor)
        asked = time.monotonic()        # o prazo do Rust começa antes de a resposta chegar aqui
        request = {'kind':'control', 'control':'keyboard_loan', 'payload':{'seconds':_LOAN_S}}
        request_id = 'admin:' + uuid.uuid4().hex
        try:
            loan = await coordinator.op(name, request, request_id)
        except Exception as exc:
            if getattr(exc, '_transport_lost', False):
                await _return_lost_loan(coordinator, name, descriptor, request, request_id)
            raise
        if loan.get('disposition') != 'accepted':
            raise TerminalControlError('keyboard_loan', loan.get('disposition'), (loan.get('payload') or {}).get('code'))
        failure = None
        try:
            deadline = asked + int(loan['payload']['seconds']) - _LOAN_MARGIN_S
            def execute():
                with borrowed(coordinator, descriptor, deadline):
                    return action()
            result = await asyncio.to_thread(execute)
            await asyncio.to_thread(validate_binding, descriptor)
        except BaseException as exc:
            failure = exc
        try:
            back = await coordinator.op(name, {'kind':'control', 'control':'keyboard_return',
                'payload':{'loan_id':(loan.get('payload') or {}).get('loan_id')}}, 'admin:' + uuid.uuid4().hex)
        except Exception as exc:
            # Devolução sem resposta: o prazo devolve o teclado ao Rust sozinho.
            diag.registrar('runtime.keyboard_return_failed', 'erro', sessao=name, **failure_reason(exc))
            back = {'disposition':'unknown'}
        if failure is not None:
            raise failure
        if back.get('disposition') != 'accepted':
            raise KeyboardLoanExpired('o Rust retomou o pane antes do fim da operação')
        return result


async def run_admin(coordinator, name, operation, payload, action, *, guest=False):
    if not await coordinator.prepare_session(name, 'claude') or not coordinator.slot(name).binding.meta.get('terminal'):
        return await asyncio.to_thread(action)
    if coordinator.slot(name).phase == Phase.Rust:
        task = asyncio.create_task(_borrow_keyboard(coordinator, name, action, guest=guest))
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            await task
            raise
    async def perform():
        async with coordinator.freeze(name):
            slot = coordinator.slot(name)
            if slot.phase != Phase.Python:
                raise RuntimeError('a sessão mudou de dono antes da administração; tente de novo')
            descriptor = slot.binding.descriptor()
            await asyncio.to_thread(validate_binding, descriptor)
            operation_id = 'admin:' + uuid.uuid4().hex
            def execute():
                with writer(coordinator, descriptor):
                    slot.store.exec(descriptor['generation'], uuid.uuid4().hex, _clock(),
                        {'kind':'prepare','id':operation_id,'payload':{'operation_id':operation_id,'kind':operation,'payload':payload},'entry_id':None})
                    slot.store.exec(descriptor['generation'], uuid.uuid4().hex, _clock(),
                        {'kind':'begin_dispatch','id':operation_id,'wire_id':'terminal-admin:' + operation_id})
                    try:
                        result = action()
                        status = 'accepted'
                    except BaseException:
                        status = 'unknown'
                        raise
                    finally:
                        slot.store.exec(descriptor['generation'], uuid.uuid4().hex, _clock(),
                            {'kind':'finish','id':operation_id,'status':status,
                             'result':{'operation_id':operation_id,'disposition':status,'payload':{}}})
                    return result
            result = await asyncio.to_thread(execute)
            await asyncio.to_thread(validate_binding, descriptor)
            return result
    task = asyncio.create_task(perform())
    try:
        return await asyncio.shield(task)
    except asyncio.CancelledError:
        await task
        raise


def _queue(coordinator, descriptor, action):
    slot = coordinator.slots[descriptor['key']]
    store = slot.store
    with store._lock:
        with slot.guard:
            if slot.store is not store or slot.binding.generation != descriptor['generation'] or slot.lease is None or slot.lease.closed:
                raise RuntimeError('reserva perdeu a posse do diário')
        sequence = max(store.state['next_seq'], max((int(key.rsplit(':',1)[1]) for key in store.state['operations']
            if key.startswith('call::terminal:queue:') and key.rsplit(':',1)[1].isdigit()), default=0)+1)
        return store.exec(descriptor['generation'], f'terminal:queue:{sequence}', _clock(), action)


def _service(coordinator, descriptor, operation_id, request_id, kind, payload):
    from app import runtime_policy
    phase = 'reserve-policy:' + uuid.uuid4().hex
    _queue(coordinator, descriptor, {'kind':'prepare','id':phase,
        'payload':{'kind':kind,'request_id':request_id,'payload':payload},'entry_id':None})
    _queue(coordinator, descriptor, {'kind':'begin_dispatch','id':phase,'wire_id':phase})
    metadata = {**descriptor['meta'], 'provider':'claude', 'name':descriptor['name'], 'key':descriptor['key'],
        'generation':descriptor['generation'], 'jsonl':descriptor['jsonl'], 'descriptor':descriptor,
        'operation_id':request_id, 'validate':lambda:assert_writer(descriptor['name'])}
    try:
        result = runtime_policy.run(kind, payload, metadata)
    except BaseException:
        _queue(coordinator, descriptor, {'kind':'finish','id':phase,'status':'unknown','result':None})
        raise
    _queue(coordinator, descriptor, {'kind':'finish','id':phase,'status':'accepted','result':result})
    return result


def _native_send(native, text, validate):
    body = text.strip('\n')
    content = (f'<cross-session-message from="{native["origin"]}" from-name="{native["sender"]}" '
        f'from-mode="{native["mode"]}">\n{body}\n</cross-session-message>')
    frame = {'msgV':1,'msg_id':native['message_id'],'type':'user', 'message':{'role':'user','content':content},
        'priority':'next','from':native['origin']}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(3)
        validate()
        try:
            stream.connect(native['socket'])
        except OSError:
            return 'not_written'
        validate()
        try:
            stream.sendall((json.dumps(frame, ensure_ascii=False)+'\n').encode())
            stream.shutdown(socket.SHUT_WR)
        except OSError:
            return 'unknown'
    return 'written'


def _reply(operation_id, disposition, **payload):
    return {'operation_id':operation_id,'disposition':disposition,'payload':payload}


# Quanto a trava do /clear espera a conversa nova antes de conferir se ele foi aplicado (a regra do Rust).
CLEAR_APPLY_WAIT_S = 10.0
# Até quando um transcript novo começado por /clear segura a trava; depois só o vínculo vale (o
# arquivo pode ser de outra sessão na mesma pasta).
CLEAR_DISK_TRUST_S = 60.0
# Próxima conferência por sessão: com o Claude ocupado, cada uma custaria três gravações do diário.
_CLEAR_NEXT: dict[str, float] = {}


def _clear_on_disk(jsonl, since):
    """Prova no disco de que o /clear rodou: transcript nascido depois do despacho, na pasta da
    conversa, que começa pelo registro do comando. Leitura que falha conta como prova."""
    atual = Path(jsonl)
    try:
        for p in atual.parent.glob('*.jsonl'):
            try:
                info = p.stat()
                # Sem data de nascimento (Linux), vale a de escrita: o teto de 60 s limita o engano.
                if p == atual or getattr(info, 'st_birthtime', info.st_mtime) < since - 1:
                    continue
                with p.open('rb') as f:
                    if b'<command-name>/clear</command-name>' in f.read(16 * 1024):
                        return True
            except FileNotFoundError:
                continue        # apagado no meio da varredura
    except OSError:
        _log.warning("trava do /clear: transcript ilegível em %s; a trava fica", atual.parent, exc_info=True)
        return True
    return False


def _expire_clear(coordinator, descriptor, operation_id):
    """Saída da trava do /clear: passado o prazo com a sessão parada e sem conversa nova (nem no vínculo
    nem no disco), o /clear não foi aplicado; com o Claude trabalhando ele espera na fila do próprio
    Claude Code. A trava sai, a operação fica recusada e nada é reenviado. True = soltou."""
    from app import diag
    slot = coordinator.slots[descriptor['key']]
    barrier = slot.store.state['runtime_state'].get('clear_barrier') or {}
    agora = time.time()
    if 'raised' not in barrier:
        # Trava gravada sem hora (de antes desta regra): o prazo conta de quando ela foi vista.
        state = copy.deepcopy(slot.store.state)
        state['runtime_state']['clear_barrier'] = {**barrier, 'since':barrier.get('since', agora), 'raised':agora}
        slot.store._persist(state)
        return False
    since, raised = barrier.get('since', barrier['raised']), barrier['raised']
    if agora - raised < CLEAR_APPLY_WAIT_S or agora < _CLEAR_NEXT.get(descriptor['key'], 0):
        return False
    _CLEAR_NEXT[descriptor['key']] = agora + CLEAR_APPLY_WAIT_S
    try:
        current = _service(coordinator, descriptor, operation_id, operation_id, 'terminal_facts',
            {'binding':descriptor['meta']['terminal'],'operation_id':operation_id,'text':''})
    except BindingChanged:
        return False        # a conversa nova existe, falta reabrir
    except Exception:
        _log.warning("trava do /clear de %s fica: fatos indisponíveis", descriptor['name'], exc_info=True)
        return False
    if not current['idle'] or agora - raised < CLEAR_DISK_TRUST_S and _clear_on_disk(descriptor['jsonl'], since):
        return False
    # Primeiro a operação: falhando, a trava fica e o diário diz por quê.
    op_id = barrier.get('operation_id')
    try:
        if op_id in slot.store.state['operations']:
            _queue(coordinator, descriptor, {'kind':'finish','id':op_id,'status':'rejected',
                'result':_reply(op_id, 'rejected', code='clear_not_applied', cleanup='not_needed')})
        state = copy.deepcopy(slot.store.state)
        state['runtime_state'].pop('clear_barrier', None)
        state['runtime_state'].pop('preserve_binding', None)
        slot.store._persist(state)
    except Exception as exc:
        from app.runtime_coordinator import failure_reason
        diag.registrar('runtime.clear_release_failed', 'erro', sessao=descriptor['name'], **failure_reason(exc))
        return False
    _CLEAR_NEXT.pop(descriptor['key'], None)
    diag.registrar('runtime.clear_not_applied', 'aviso', sessao=descriptor['name'], codigo='clear_not_applied')
    return True


def _python_prompt(name, text):
    from app import terminal_input as ti
    for field in ('limpou', 'stage'):
        if hasattr(ti._ULTIMA_LIMPEZA, field):
            delattr(ti._ULTIMA_LIMPEZA, field)
    try:
        result = ti.TerminalInput().send_prompt(name, text)
        return result, getattr(ti._ULTIMA_LIMPEZA, 'limpou', False), getattr(ti._ULTIMA_LIMPEZA, 'stage', '') or ''
    finally:
        for field in ('limpou', 'stage'):
            if hasattr(ti._ULTIMA_LIMPEZA, field):
                delattr(ti._ULTIMA_LIMPEZA, field)


def _finish_native(coordinator, descriptor, action):
    state = coordinator.slots[descriptor['key']].store.state
    root, result = action.get('id'), action.get('result')
    if not isinstance(result,dict) or set(result) != {'operation_id','disposition','payload'}:
        raise ValueError('recibo nativo inválido')
    body = result['payload']
    if not isinstance(body,dict) or set(body) != {'native_status'}:
        raise ValueError('recibo nativo sem status')
    status = body['native_status']
    disposition = 'accepted' if status in {'delivered','released',''} else 'rejected' if status in {'refused','rejected'} else 'unknown'
    if result['operation_id'] != root or result['disposition'] != disposition or action['status'] != disposition:
        raise ValueError('recibo nativo não corresponde à entrada')
    expected = str(uuid.uuid5(uuid.NAMESPACE_URL, f"hangar:{descriptor['key']}:{root}"))
    attempts = [op for op in state['operations'].values() if op.get('entry_id') == root
        and op['payload'].get('kind') == 'input'
        and op['payload'].get('payload',{}).get('_terminal_generation') == descriptor['generation']
        and (op.get('result') or {}).get('payload',{}).get('native') is True
        and (op.get('result') or {}).get('payload',{}).get('message_id') == expected]
    if not attempts or not any(row['id'] == root for row in state['rows']):
        raise ValueError('recibo sem tentativa nativa')
    delivery = {**attempts[-1]['result']['payload'],'native_status':status}
    for op in attempts:
        if op['status'] != 'confirmed':
            _queue(coordinator, descriptor, {'kind':'finish','id':op['id'],'status':disposition,
                'result':_reply(op['id'],disposition,**delivery)})
    return _reply(root,disposition,**delivery)


def _validate_control(control, payload):
    from app.terminal_input import TerminalInput, _validate
    allowed = {'select':{'option','require_cursor','request_id'}, 'answer_questions':{'answers','request_id'},
        'key':{'key'}, 'navigation_key':{'key'}, 'interactive_key':{'key'}, 'terminal_input':{'text'},
        'interrupt':{'clear'}, 'submit_selected':set(), 'steer':set(), 'steer_queue':{'entry_id'}}
    if control not in allowed or set(payload) - allowed[control]:
        raise ValueError('controle terminal inválido')
    if control == 'select' and (type(payload.get('option')) is not int or not 1 <= payload['option'] <= 100):
        raise ValueError('opção inválida')
    if control in {'key','navigation_key','interactive_key'}:
        keys = TerminalInput._TERM_KEYS if control == 'interactive_key' else TerminalInput._NAV_KEYS
        if payload.get('key') not in keys:
            raise ValueError('tecla não permitida')
    if control == 'answer_questions':
        if not isinstance(payload.get('answers'), list) or not payload['answers']:
            raise ValueError('resposta terminal vazia')
        _validate(payload['answers'])
        for answer in payload['answers']:
            if (set(answer) - {'kind','question_id','indices','labels','multi','value','type_index','chat_index'}
                    or answer.get('question_id') is not None and not isinstance(answer['question_id'],str)):
                raise ValueError('identidade da resposta inválida')
            if answer['kind'] == 'option' and (not answer.get('labels')
                    or any(type(index) is not int or not 0 <= index < 100 for index in answer['indices'])
                    or not answer.get('multi') and len(answer['indices']) != 1):
                raise ValueError('opção fora da pergunta')
            if answer['kind'] in {'text','chat'}:
                field = 'type_index' if answer['kind'] == 'text' else 'chat_index'
                if type(answer.get(field)) is not int or not 0 <= answer[field] < 100:
                    raise ValueError('posição fora da pergunta')
            if answer['kind'] == 'text':
                _validate_text(answer['value'])
                if '\n' in answer['value']:
                    raise ValueError('resposta terminal com quebra de linha')
    if payload.get('request_id') is not None and not isinstance(payload['request_id'],str):
        raise ValueError('pedido terminal sem identidade válida')
    if control == 'terminal_input':
        _validate_text(payload.get('text'), interactive=True)


def _validate_text(text, interactive=False):
    import unicodedata
    if not isinstance(text, str) or not interactive and not text.strip() or any(
            unicodedata.category(c) == 'Cc' and c not in '\n\t' for c in text):
        raise ValueError('texto inválido para terminal')


def _reserve_execute(coordinator, descriptor, command, operation_id, *, entry_id=None):
    from app import terminal_input as ti, tmux
    from app.runtime_receipt import ReceiptIndex
    slot = coordinator.slots[descriptor['key']]
    kind = command['kind']
    if kind == 'submit':
        payload = {'text':command['text'], 'pre_transcript':command.get('pre_transcript', False)}
        _validate_text(payload['text'])
        control = 'input'
        slash = payload['text'].lstrip().startswith('/')
        entry_id = None if slash else entry_id or operation_id
        if not slash:
            payload['_terminal_generation'] = descriptor['generation']
    elif kind == 'control':
        control, payload = command['control'], copy.deepcopy(command.get('payload') or {})
        _validate_control(control, payload)
        slash = False
    else:
        raise ValueError('operação terminal inválida')
    intent = {'operation_id':operation_id,'kind':control,'payload':payload}
    old = copy.deepcopy(slot.store.state['operations'].get(operation_id))
    if old:
        if old['payload'] != intent:
            raise ValueError('operação repetida com outro corpo')
        if isinstance(old['result'], dict) and 'disposition' in old['result']:
            return old['result']
        return _reply(operation_id, 'deferred' if old['status'] in {'prepared','deferred'} else 'unknown', cleanup='not_needed')
    barrier = slot.store.state['runtime_state'].get('clear_barrier')
    if barrier and barrier['generation'] == descriptor['generation'] and not _expire_clear(coordinator, descriptor, operation_id):
        raise RuntimeError('clear exige geração nova antes de escrever')
    prepared = _queue(coordinator, descriptor, {'kind':'prepare','id':operation_id,'payload':intent,'entry_id':entry_id})
    if prepared['status'] in {'accepted','confirmed','rejected'}:
        return prepared.get('result') or _reply(operation_id, 'rejected' if prepared['status'] == 'rejected' else 'accepted')
    if entry_id and not any(row['id'] == entry_id for row in slot.store.state['rows']):
        _queue(coordinator, descriptor, {'kind':'append','text':payload['text'],'delivered':False,
            'ts':None,'pre_transcript':payload['pre_transcript'],'entry_id':entry_id})
    root = entry_id or operation_id
    binding = descriptor['meta']['terminal']
    from app.runtime_queue import terminal_write_blocked
    if control == 'input' and payload['text'].split()[:1] != ['/clear'] and terminal_write_blocked(slot.store.state, binding['conversation']):
        result = _reply(operation_id, 'deferred', code='terminal_write_barrier', queued=bool(entry_id), cleanup='not_needed')
        _queue(coordinator, descriptor, {'kind':'finish','id':operation_id,'status':'deferred','result':result})
        return result
    dispatched = False
    dispatched_at = time.time()     # antes da escrita: o transcript novo do /clear nasce logo depois do Enter
    def begin():
        nonlocal dispatched
        if dispatched:
            return
        if entry_id:
            cursor = ReceiptIndex('claude', binding['conversation']).capture(descriptor['jsonl'])
            _queue(coordinator, descriptor, {'kind':'bind_dispatch','id':operation_id,'cursor':cursor})
            _queue(coordinator, descriptor, {'kind':'set_delivered','entry_id':entry_id,'value':True,'steered':False})
        _queue(coordinator, descriptor, {'kind':'begin_dispatch','id':operation_id,
            'wire_id':f"terminal:{descriptor['generation']}:{operation_id}"})
        dispatched = True
    try:
        if control == 'input':
            current = _service(coordinator, descriptor, operation_id, root, 'terminal_facts',
                {'binding':binding,'operation_id':root,'text':payload['text']})
            if (not current['ready'] or current['open_question']) and not (current.get('native') and not slash):
                result = _reply(operation_id, 'deferred', cleanup='not_needed')
            else:
                begin()
                native = current.get('native')
                outcome = _native_send(native, payload['text'], lambda:assert_writer(descriptor['name'])) if native and not slash else 'not_written'
                if outcome != 'not_written':
                    result = _reply(operation_id, 'accepted' if outcome == 'written' else 'unknown', native=True,
                        message_id=native['message_id'], cleanup='not_needed')
                else:
                    if not current['ready'] or current['open_question']:
                        result = _reply(operation_id, 'deferred', cleanup='not_needed')
                        _queue(coordinator, descriptor, {'kind':'finish','id':operation_id,'status':'deferred','result':result})
                        return result
                    published = 'unavailable'
                    before_publish = None
                    if current['plugin_live'] and not slash:
                        before_publish = _composer_snapshot(descriptor['name'])
                        mode = 'user' if current['plugin_user'] and current['idle'] and not any(c in payload['text'] for c in '@!') else 'fill'
                        published = _service(coordinator, descriptor, operation_id, root, 'terminal_publish',
                            {'binding':binding,'operation_id':root,'publication':{'id':f"{descriptor['key']}:{descriptor['generation']}:{operation_id}",
                                'text':payload['text'],'mode':mode},'generation':descriptor['generation']})
                    if published in {'accepted','unknown'}:
                        result = _reply(operation_id, published, cleanup='not_needed')
                    elif published == 'filled':
                        if (not _proves_input(_composer_snapshot(descriptor['name']), payload['text'], before_publish)
                                or not tmux.send_keys(descriptor['name'], 'Enter')
                                or not _proved_submission(descriptor['name'])):
                            result = _reply(operation_id, 'unknown', cleanup='unproved')
                        else:
                            result = _reply(operation_id, 'accepted', cleanup='not_needed')
                    else:
                        # O driver antigo só é acessível dentro da unidade registrada e autorizada.
                        sent, clean, stage = _python_prompt(descriptor['name'], payload['text'])
                        if sent == 'sent':
                            result = _reply(operation_id, 'accepted', cleanup='not_needed')
                        elif sent == 'deferred':
                            result = _reply(operation_id, 'deferred', cleanup='not_needed')
                        else:
                            safe = clean and not stage.endswith('submeter')
                            result = _reply(operation_id, 'deferred' if safe else 'unknown', cleanup='proved' if safe else 'unproved',
                                **({'stage':stage} if stage else {}))
        else:
            begin()
            held = payload.get('request_id', '')
            result = None
            if control in {'select','answer_questions'} and held.startswith(('perm:','ask:')):
                plugin = _service(coordinator, descriptor, operation_id, held, 'terminal_plugin_control',
                    {'binding':binding,'operation_id':root,'control':control,'payload':payload,
                        'publication_id':f"{descriptor['key']}:{descriptor['generation']}:{operation_id}", 'generation':descriptor['generation']})
                if plugin['disposition'] != 'unavailable':
                    result = _reply(operation_id, plugin['disposition'])
            if result is None:
                terminal = ti.TerminalInput()
                methods = {'key':terminal.send_key,'navigation_key':terminal.send_key,'interactive_key':terminal.send_term_key,
                    'terminal_input':terminal.send_text,'select':terminal.select,'interrupt':terminal.interrupt,
                    'submit_selected':terminal.submeter_multipla,'answer_questions':ti.answer_questions,
                    'steer':lambda name:ti.steer_now(name,'claude'), 'steer_queue':lambda name:ti.steer_now(name,'claude')}
                params = {key:value for key,value in payload.items() if key not in {'request_id','entry_id'}}
                value = methods[control](descriptor['name'], **params)
                if value is False:
                    result = _reply(operation_id, 'unknown')
                else:
                    result = _reply(operation_id, 'accepted', promoted=value != 'sem-fila')
    except BaseException as exc:
        result = _reply(operation_id, 'unknown' if dispatched else 'deferred', cleanup='unproved' if dispatched else 'not_needed')
        _queue(coordinator, descriptor, {'kind':'finish','id':operation_id,'status':result['disposition'],'result':result})
        if dispatched and isinstance(exc, Exception):
            raise TerminalOutcomeUnknown('resultado terminal incerto; efeito conservado sem fallback') from exc
        raise
    # Só o /clear cujo Enter pode ter saído ergue a trava; incerto antes do Enter é entrega incerta comum.
    stage = (result.get('payload') or {}).get('stage') or ''
    if control == 'input' and payload['text'].strip().split()[0] == '/clear' and (result['disposition'] == 'accepted'
            or result['disposition'] == 'unknown' and (not stage or stage.endswith('submeter'))):
        _CLEAR_NEXT.pop(descriptor['key'], None)
        state = copy.deepcopy(slot.store.state)
        state['runtime_state'].update(preserve_binding=True, clear_barrier={'generation':descriptor['generation'],
            'conversation':binding['conversation'],'operation_id':operation_id,'since':dispatched_at,'raised':time.time()})
        slot.store._persist(state)
        result['payload']['preserve_binding'] = True
    _queue(coordinator, descriptor, {'kind':'finish','id':operation_id,'status':result['disposition'],'result':result})
    return result


async def reserve_op(coordinator, descriptor, command, operation_id):
    async def execute():
        from app.runtime_adapter import LegacyBridge
        def run():
            with writer(coordinator, descriptor):
                kind = command['kind']
                if kind == 'ensure_projection':
                    return _queue(coordinator, descriptor, {'kind':'ensure_projection'})
                if kind == 'snapshot':
                    return copy.deepcopy(coordinator.slot(descriptor['name']).view)
                if kind == 'queue':
                    if command['action']['kind'] == 'finish':
                        return _finish_native(coordinator, descriptor, command['action'])
                    action = command['action']
                    if action['kind'] in {'claim','bump_attempts','reconcile','replace_rows','prepare','begin_dispatch',
                            'bind_dispatch','recover','confirm','confirm_occurrence','set_runtime_state','late_rpc_resolution'} or (
                            action['kind'] == 'set_delivered' and action.get('value') is False):
                        raise RuntimeError('ação privada do ator terminal; confirmação exige transcript')
                    return _queue(coordinator, descriptor, command['action'])
                if kind == 'drain':
                    state = coordinator.slot(descriptor['name']).store.state
                    from app.runtime_queue import terminal_write_blocked
                    if terminal_write_blocked(state, descriptor['meta']['terminal']['conversation']):
                        return {'sent':0}
                    if (state['runtime_state'].get('clear_barrier', {}).get('generation') == descriptor['generation']
                            and not _expire_clear(coordinator, descriptor, operation_id)):
                        return {'sent':0}
                    binding = descriptor['meta']['terminal']
                    current = _service(coordinator, descriptor, operation_id, operation_id, 'terminal_facts',
                        {'binding':binding,'operation_id':operation_id,'text':''})
                    if not current['ready'] or not current['idle'] or current['open_question']:
                        return {'sent':0}
                    rows = _queue(coordinator, descriptor, {'kind':'claim','min_ts':descriptor['meta'].get('created', binding['created']),'limit':1,'entry_id':None})
                    sent = 0
                    for row in rows:
                        attempt = 'reserve-input:' + uuid.uuid4().hex
                        result = _reserve_execute(coordinator, descriptor,
                            {'kind':'submit','text':row['text'],'pre_transcript':row.get('pre_transcript',False)}, attempt, entry_id=row['id'])
                        sent += result['disposition'] == 'accepted'
                    return {'sent':sent}
                return _reserve_execute(coordinator, descriptor, command, operation_id)
        if command['kind'] in {'confirm','drain'}:
            # Confirmação continua por ocorrência no transcript, inclusive sem aparelho.
            confirmed = await LegacyBridge(coordinator, {}).confirm(descriptor)
            if command['kind'] == 'confirm':
                return confirmed
        return await asyncio.to_thread(run)
    task = asyncio.create_task(execute())
    try:
        return await asyncio.shield(task)
    except asyncio.CancelledError:
        await task
        raise


async def route(coordinator, name, command):
    if not await coordinator.prepare_session(name, 'claude'):
        return None
    slot = coordinator.slot(name)
    if not slot.binding.meta.get('terminal'):
        return None
    if command['kind'] == 'control':
        _validate_control(command['control'], command.get('payload') or {})
    result = await coordinator.op(name, command, uuid.uuid4().hex)
    if command['kind'] == 'control' and result.get('disposition') != 'accepted':
        raise TerminalControlError(command['control'], result.get('disposition'), (result.get('payload') or {}).get('code'))
    if result.get('disposition') == 'rejected':
        raise RuntimeError(f"entrada recusada pelo terminal ({(result.get('payload') or {}).get('code') or 'sem código'})")
    if result.get('disposition') not in {None,'accepted','deferred'}:
        raise TerminalOutcomeUnknown('resultado terminal incerto; não repetir por outro transporte')
    return result


def route_sync(name, command):
    from app import runtime_coordinator
    from app.runtime_adapter import run_sync
    coordinator = runtime_coordinator.current()
    if coordinator is None or coordinator.legacy is None or _writer.get() is not None:
        return None
    return run_sync(lambda:route(coordinator, name, command), coordinator.loop)


def wrap_driver(original, *, control=None, admin=False):
    import functools
    import inspect
    signature = inspect.signature(original)
    @functools.wraps(original)
    def wrapped(*args, **kwargs):
        from app import runtime_coordinator
        from app.runtime_adapter import run_sync
        arguments = signature.bind(*args, **kwargs)
        arguments.apply_defaults()
        name = arguments.arguments.get('name')
        provider = arguments.arguments.get('provider','claude')
        if provider != 'claude' or _writer.get() is not None:
            return original(*args, **kwargs)
        owner = runtime_coordinator.current()
        if owner is None or owner.legacy is None:
            return original(*args, **kwargs)
        payload = {key:value for key,value in arguments.arguments.items() if key not in {'self','name','provider','pane_id','msg_id','jsonl'}}
        if admin:
            # Lida aqui, na thread de quem pede: a tarefa no loop do coordenador nasce com outro contexto.
            guest = guest_admin.get()
            return run_sync(lambda:run_admin(owner, name, original.__name__, payload,
                lambda:original(*args, **kwargs), guest=guest), owner.loop)
        if control == 'submit':
            command = {'kind':'submit','text':payload['text']}
        elif control == 'drain':
            command = {'kind':'drain'}
        else:
            command = {'kind':'control','control':control,'payload':payload}
        result = route_sync(name, command)
        if result is None:
            return original(*args, **kwargs)
        if control == 'submit':
            return 'sent' if result['disposition'] == 'accepted' else 'deferred'
        if control == 'drain':
            return result.get('sent',0)
        if control == 'steer':
            return True if result['disposition'] == 'accepted' and result.get('payload',{}).get('promoted',True) else 'sem-fila'
        return None
    return wrapped


class ClipboardLock:
    def __init__(self):
        import threading
        self.local = threading.RLock()
        self.depth = threading.local()

    def __enter__(self):
        self.local.acquire()
        try:
            if getattr(self.depth, 'count',0) == 0:
                deadline = time.monotonic()+5
                while True:
                    try:
                        self.depth.lease = WriterLease(_clipboard_lock_path())
                        break
                    except BlockingIOError:
                        if time.monotonic() >= deadline:
                            raise RuntimeError('clipboard ainda possui outro escritor')
                        time.sleep(.02)
            self.depth.count = getattr(self.depth,'count',0)+1
            return self
        except BaseException:
            self.local.release()
            raise

    def __exit__(self, *exc):
        self.depth.count -= 1
        if self.depth.count == 0:
            self.depth.lease.close()
        self.local.release()


def answer_sync(name, answers, request_id, jsonl):
    from app import runtime_coordinator, api, terminal_input as ti, plugin_bridge
    from app.runtime_adapter import run_sync
    owner = runtime_coordinator.current()
    if owner is None or owner.legacy is None or _writer.get() is not None:
        return None
    payload = {'answers':[{**answer, 'indices':answer.get('indices') or []} for answer in answers]}
    if request_id:
        payload['request_id'] = request_id
    async def perform():
        if not await owner.prepare_session(name,'claude') or not owner.slot(name).binding.meta.get('terminal'):
            return None
        ti._validate(answers)
        pending = await asyncio.to_thread(plugin_bridge.pergunta_pendente, name)
        if pending:
            current_id = pending['id']
            if request_id and request_id not in {current_id, current_id.removeprefix('ask:')}:
                raise ValueError('a pergunta mudou; resposta conservada')
            payload['request_id'] = current_id if current_id.startswith(('ask:', 'perm:')) else 'ask:' + current_id
        if any(answer.get('kind') == 'chat' for answer in answers) and pending is None:
            text = await asyncio.to_thread(api._askq_conversar_text, answers, jsonl)
            if not text:
                raise ValueError('resposta sem texto para conversar')
            if owner.slot(name).phase == Phase.Rust:
                # Teclado emprestado só para fechar a pergunta; o texto entra pela fila do Rust.
                def close_question():
                    ti.TerminalInput().interrupt(name)
                    api._espera_picker_fechar(name)
                await run_admin(owner, name, 'answer_chat', payload, close_question)
                # `owner.op` direto: o teclado já voltou ao Rust (o empréstimo acabou em `run_admin`), e `route`
                # transformaria `unknown`/`rejected` em exceções de tipos diferentes.
                # Na fila do Rust (`deferred`) a resposta sai quando o terminal ficar livre.
                reply = await owner.op(name, {'kind':'submit','text':text}, uuid.uuid4().hex)
                if reply.get('disposition') not in {'accepted', 'deferred'}:
                    # 502, não 409: o app trata 409 como "nada digitado", e aqui a pergunta já fechou e o texto pode ter entrado.
                    raise RuntimeError('a pergunta foi fechada, mas a resposta por texto não foi confirmada — confira na sessão antes de responder de novo')
                return reply
            def compound():
                ti.TerminalInput().interrupt(name)
                api._espera_picker_fechar(name)
                descriptor = owner.slot(name).binding.descriptor()
                reply = _reserve_execute(owner, descriptor, {'kind':'submit','text':text}, uuid.uuid4().hex)
                if reply['disposition'] != 'accepted':
                    raise RuntimeError('resposta por texto não foi confirmada; pergunta conservada')
                return reply
            return await run_admin(owner, name, 'answer_chat', payload, compound)
        return await route(owner, name, {'kind':'control','control':'answer_questions','payload':payload})
    return run_sync(perform, owner.loop)


def _clipboard_lock_path():
    from app.pqueue import _queue_dir
    return _queue_dir() / 'runtime' / 'clipboard.lock'


def import_legacy(coordinator, binding, projection):
    from app import atomico, diag
    import tempfile
    # A projeção por nome não possui identidade; a cópia é anterior à primeira sobrescrita.
    data = projection.read_bytes()
    backup = binding.state_path.with_suffix('.legacy.jsonl')
    if not backup.exists():
        with tempfile.NamedTemporaryFile(dir=backup.parent, delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            atomico.substituir(temporary, backup)
        finally:
            temporary.unlink(missing_ok=True)
    elif backup.read_bytes() != data:
        raise RuntimeError('fila Legacy mudou depois da cópia; projeção conservada')
    validate_binding(binding.descriptor())
    rows = [json.loads(line) for line in data.decode('utf-8').splitlines()]
    if any(not isinstance(row,dict) for row in rows):
        raise ValueError('fila Legacy inválida; cópia conservada')
    previous_life = any(slot.binding.name == binding.name and slot.binding.key != binding.key
        for slot in coordinator.slots.values())
    for path in binding.state_path.parent.glob('*.json'):
        if path != binding.state_path:
            saved = json.loads(path.read_bytes())
            previous_life = previous_life or saved.get('name') == binding.name
    cut = binding.meta.get('legacy_import_after')
    if cut is None and rows and not previous_life:
        diag.registrar('runtime.legacy_queue_blocked', 'erro', sessao=binding.name, entradas=len(rows))
        raise RuntimeError(f'fila antiga sem prova de vida: {len(rows)} entrada(s) preservada(s) em {backup}')
    accepted = [] if previous_life else [row for row in rows if isinstance(row.get('ts'),(int,float)) and row['ts'] >= cut]
    diag.registrar('runtime.legacy_queue_import', 'aviso' if len(accepted) != len(rows) else 'ok',
        sessao=binding.name, importadas=len(accepted), preservadas=len(rows)-len(accepted))
    validate_binding(binding.descriptor())
    return accepted


def _session_hash(namespace, session_id, created):
    return hashlib.sha256(json.dumps([namespace, session_id, created]).encode()).hexdigest()


def _session_proof(name):
    from app import tmux
    cp = tmux._run(['tmux','display-message','-p','-t',f'={name}:',
        '#{socket_path}\t#{pid}\t#{session_created}\t#{session_name}\t#{session_id}'])
    fields = cp.stdout.strip().split('\t')
    if cp.returncode or len(fields) != 5 or not fields[0] or not fields[1].isdigit() or not fields[2].isdigit() or fields[3] != name:
        return None
    import psutil
    try:
        namespace = f'{fields[0]}:{int(fields[1])}:{psutil.Process(int(fields[1])).create_time()}'
    except psutil.Error:
        return None     # o servidor tmux saiu entre a pergunta e a leitura do processo dele
    return _session_hash(namespace, fields[4], int(fields[2]))


def _pane_life(name):
    """Nascimento do processo do pane ativo: `respawn-pane` troca o pane sem trocar a sessão tmux."""
    from app import tmux
    cp = tmux._run(['tmux','display-message','-p','-t',f'={name}:','#{pane_pid}'])
    pid = cp.stdout.strip()
    if cp.returncode or not pid.isdigit():
        return None
    import psutil
    try:
        return f'{pid}:{psutil.Process(int(pid)).create_time()}'
    except psutil.Error:
        return None


def pending_binding(name, previous):
    proof = _session_proof(name)
    if proof is None:
        return None
    binding = copy.deepcopy(previous)
    binding.name, binding.headless = name, False
    binding.meta.pop('cano', None)
    binding.meta.update(headless=False, pending_terminal=proof)
    return binding


def terminal_life(binding):
    """Prova da vida do terminal de um vínculo Claude com pane, confirmado ou pendente."""
    if binding.provider != 'claude' or not (binding.meta.get('terminal') or binding.meta.get('pending_terminal')):
        return None
    proof = _session_proof(binding.name)
    return None if proof is None else (proof, _pane_life(binding.name))


def reborn_binding(name, previous, life_before):
    """Terminal recriado dentro de uma troca: a vida muda, a sessão não, e a chave é da sessão
    quando o pane novo roda a mesma conversa (`resolve_binding` confere)."""
    pending = pending_binding(name, previous)
    if pending is None or (pending.meta['pending_terminal'], _pane_life(name)) == life_before:
        return None
    # O pane antigo morreu com a vida antiga: nenhuma escrita pode mirar nele.
    pending.meta.pop('terminal', None)
    return resolve_binding(name, pending) or pending


def _native_record(current):
    pid = current.meta.get('agent_pid')
    if not pid:
        return None
    path = Path(current.meta['config_dir']) / 'sessions' / f'{pid}.json'
    if not path.exists():
        return None
    record = json.loads(path.read_bytes())
    if record.get('pid') != pid or record.get('sessionId') != current.meta['session_id']:
        raise RuntimeError('registro nativo mudou; operação terminal suspensa')
    return record


def _native_socket(current):
    record = _native_record(current)
    socket_path = (record or {}).get('messagingSocketPath')
    return socket_path if isinstance(socket_path, str) and socket_path and Path(socket_path).exists() else None


def _native_state(current):
    record = _native_record(current)
    return {'idle':'idle','busy':'working','waiting':'awaiting_input','shell':'idle'}.get((record or {}).get('status'))


def _plugin_current(current):
    from app import plugin_bridge
    with plugin_bridge._lock:
        owner = plugin_bridge._donos.get(current.name)
    if owner is None:
        return False
    try:
        born = int(owner[0].split('-',1)[0], 36) / 1000
    except (ValueError, TypeError):
        return False
    cut = max(current.meta.get('legacy_import_after') or current.meta.get('created', 0), current.meta.get('agent_birth') or 0)
    return born >= cut


def _composer_snapshot(name):
    from app import terminal_input as ti
    region = ti._composer_regiao(ti._capture(name), name)
    if region is None:
        return None
    content = '\n'.join(line.strip().removeprefix('❯').strip() for line in region.splitlines()[1:-1])
    return content, ti._paste_ids(region)


def _proves_input(snapshot, text, before):
    from app import terminal_input as ti
    if snapshot is None or before is None:
        return False
    content, placeholders = snapshot
    if placeholders - before[1]:
        return True
    actual, expected = ti._sem_espaco(content), ti._sem_espaco(text)
    if len(expected) < 12:
        return actual == expected and bool(expected)
    parts = (text.strip()[:40], text.strip().split('\n')[-1][-40:])
    return any(len(candidate) >= 12 and candidate in actual for candidate in map(ti._sem_espaco, parts))


def _proved_submission(name):
    from app import terminal_input as ti
    deadline = time.monotonic() + ti._SUBMIT_CHECK_PRAZO
    while True:
        snapshot = _composer_snapshot(name)
        if snapshot is not None and not snapshot[0].strip():
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(ti._SUBMIT_CHECK_INTERVALO)
