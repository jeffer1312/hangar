import asyncio
import copy
import json
import threading
import time
import sys
from pathlib import Path

import pytest

from app import runtime_coordinator as rc, runtime_adapter as ra, plugin_bridge as pb
from app.runtime_coordinator import Binding, RuntimeCoordinator, Phase, WriterLease


async def _to_rust(owner, name):
    """Registro Python do terminal aberto no Rust (o caminho do vínculo pendente que provou a conversa)."""
    await owner._open_slot_in_rust(name, owner.slot(name), launch=False)
    return True


def terminal_binding(tmp_path):
    terminal = dict(name='session', pane='%1', conversation='sid', generation=1,
        created=123, mux_argv=['tmux'], windows=False,
        clipboard_lock_path=str(tmp_path / 'clipboard.lock'))
    return Binding('session', 'terminal_key', 'claude', False,
        dict(key='terminal_key', terminal=terminal, fingerprint='life', session_id='sid', cwd=str(tmp_path)),
        str(tmp_path / 'sid.jsonl'), tmp_path, tmp_path / 'runtime' / 'terminal_key.json',
        tmp_path / 'runtime' / 'terminal_key.lock', 1)


@pytest.fixture(autouse=True)
def isolated_owner(monkeypatch):
    monkeypatch.setattr(rc, '_current', None)
    from app import runtime_queue
    monkeypatch.setattr(runtime_queue, '_coordinator', None)
    yield
    owner = rc.current()
    if owner:
        owner.close_python_leases()


def test_new_terminal_registered_without_cano_or_transcript(tmp_path):
    owner = RuntimeCoordinator()
    binding = terminal_binding(tmp_path)
    slot = owner.register(binding)
    assert slot is not None
    assert owner.managed_runtime('session')
    assert json.loads(binding.state_path.read_bytes())['runtime_state']['_binding'] == binding.descriptor()
    assert ra.native_slot('session') is None


def test_codex_terminal_and_unvalidated_terminal_remain_legacy(tmp_path):
    owner = RuntimeCoordinator()
    target = terminal_binding(tmp_path)
    target.meta.pop('terminal')
    assert owner.register(target) is None
    target.provider = 'codex'
    target.meta['terminal'] = terminal_binding(tmp_path).meta['terminal']
    assert owner.register(target) is None


def test_terminal_resolver_same_life_rename_and_new_mux_no_inherited_debt(monkeypatch, tmp_path):
    from app import runtime_terminal as terminal
    facts = dict(name='session', pane='%1', created=123, namespace='socket:pid1:born1',
        jsonl=str(tmp_path / 'sid.jsonl'), session_id='sid', config_dir=str(tmp_path),
        cwd=str(tmp_path), mux_argv=['tmux'], windows=False)
    monkeypatch.setattr(terminal, '_collect', lambda name: {**facts, 'name':name})
    from app import pqueue
    monkeypatch.setattr(pqueue, '_queue_dir', lambda:tmp_path)
    first = terminal.resolve_binding('session')
    renamed = terminal.resolve_binding('renamed', first)
    assert renamed.key == first.key
    assert renamed.meta['terminal']['name'] == 'renamed'
    facts['namespace'] = 'socket:pid2:born2'
    replacement = terminal.resolve_binding('session', first)
    assert replacement.key != first.key
    assert replacement.jsonl == first.jsonl


def test_terminal_binding_carries_the_key_of_the_launch_token(monkeypatch, tmp_path):
    # O Rust acha pela chave do token a sessão que o Hangar lançou; token só do nome não leva chave.
    from app import runtime_terminal as terminal, pqueue
    facts = dict(name='session', pane='%1', created=123, namespace='socket:pid1:born1',
        jsonl=str(tmp_path / 'sid.jsonl'), session_id='sid', config_dir=str(tmp_path),
        cwd=str(tmp_path), mux_argv=['tmux'], windows=False, plugin_token='k1.' + '0' * 32)
    monkeypatch.setattr(terminal, '_collect', lambda name: dict(facts))
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    assert terminal.resolve_binding('session').meta['plugin_key'] == 'k1'
    facts['plugin_token'] = '0' * 32
    assert 'plugin_key' not in terminal.resolve_binding('session').meta


def test_terminal_native_slot_never_replaces_2c_observation(tmp_path):
    owner = RuntimeCoordinator()
    slot = owner.register(terminal_binding(tmp_path))
    assert slot is not None
    slot.phase = Phase.Rust
    slot.lease.close()
    slot.lease = None
    assert ra.native_slot('session') is None


def test_plugin_old_capability_refuses_before_enqueue(monkeypatch):
    pb._waiters['session'] = asyncio.Queue()
    pb._donos['session'] = ('i', {'fill','user'}, time.monotonic())
    monkeypatch.setattr(pb, 'tracked_session_id', lambda name:'sid')
    try:
        assert pb.publish_terminal('session', 'sid', 1, {'id':'pub', 'text':'hello','mode':'fill'}, lambda:None) == 'not_written'
        assert pb._waiters['session'].empty()
    finally:
        pb._waiters.clear()
        pb._donos.clear()


@pytest.mark.parametrize('ack', ['old', 'uncorrelated', 'wrong_generation', 'wrong_conversation', 'matching'])
def test_plugin_receipt_correlates_publication_generation_conversation(monkeypatch, ack):
    monkeypatch.setattr(pb, 'PUBLICA_S', .06)
    monkeypatch.setattr(pb, 'tracked_session_id', lambda name:'sid')
    async def flow():
        pb._loop = asyncio.get_running_loop()
        pb._waiters['session'] = asyncio.Queue()
        pb._donos['session'] = ('i', {'fill','user','receipt_v2'}, time.monotonic())
        task = asyncio.create_task(asyncio.to_thread(pb.publish_terminal, 'session', 'sid', 1,
            {'id':'new', 'text':'hello','mode':'fill'}, lambda:None))
        payload = await asyncio.wait_for(pb._waiters['session'].get(), .5)
        assert payload['publication_id'] == 'new'
        extra = {'publication_id':'new','generation':1,'session_id':'sid'}
        if ack == 'old': extra['publication_id'] = 'old'
        if ack == 'uncorrelated': extra = {}
        if ack == 'wrong_generation': extra['generation'] = 0
        if ack == 'wrong_conversation': extra['session_id'] = 'old-sid'
        await pb.filled(pb.FilledBody(sessao='session', token=pb.mint('session'), ok=True, **extra))
        assert await task == ('filled' if ack == 'matching' else 'unknown')
        if ack != 'matching':
            assert pb._publications['session']['id'] == 'new'
    try:
        asyncio.run(flow())
    finally:
        pb._publications.clear()
        pb._waiters.clear()
        pb._donos.clear()


def test_rust_alive_or_missing_descendant_proof_never_recovers_terminal(tmp_path):
    async def flow():
        owner = RuntimeCoordinator()
        slot = owner.register(terminal_binding(tmp_path))
        assert slot
        slot.phase = Phase.Rust
        slot.lease.close()
        slot.lease = None
        with pytest.raises(RuntimeError):
            await owner.recover('session', confirmed_dead=True)
        assert slot.phase == Phase.Rust
    asyncio.run(flow())


def test_tmux_writer_blocked_while_rust_owns_terminal(monkeypatch, tmp_path):
    from app import tmux
    owner = RuntimeCoordinator()
    slot = owner.register(terminal_binding(tmp_path))
    assert slot
    slot.phase = Phase.Rust
    slot.lease.close()
    slot.lease = None
    calls = []
    monkeypatch.setattr(tmux, '_run', lambda *args, **kwargs:calls.append(args))
    with pytest.raises(RuntimeError): tmux.send_keys('session', 'Enter')
    with pytest.raises(RuntimeError): tmux.paste_text('session', 'hello')
    assert calls == []


def live_owner(monkeypatch, tmp_path, *, gateway=None):
    from app import runtime_terminal as terminal, pqueue, terminal_input as ti
    from app.adapters.claude_headless import sessions
    from app.runtime_adapter import LegacyBridge
    collected = dict(name='session',pane='%1',created=1,namespace='socket:pid:birth',
        jsonl=str(tmp_path / 'sid.jsonl'),session_id='sid',config_dir=str(tmp_path),cwd=str(tmp_path),
        mux_argv=['tmux'],windows=False)
    monkeypatch.setattr(terminal, '_collect',lambda name:{**collected,'name':name})
    monkeypatch.setattr(pqueue,'_queue_dir',lambda:tmp_path)
    monkeypatch.setattr(sessions,'load',lambda name:None)
    monkeypatch.setattr(ti,'_capture',lambda name:'─'*30+'\n❯ \n'+'─'*30+'\n')
    monkeypatch.setattr(ti,'classify',lambda pane:('idle',None,None,None))
    monkeypatch.setattr(ti,'is_overlay',lambda pane:False)
    monkeypatch.setattr(pb,'pergunta_pendente',lambda name:None)
    owner = RuntimeCoordinator(gateway)
    owner.legacy = LegacyBridge(owner, {'claude':object()})
    target = terminal.resolve_binding('session')
    slot = owner.register(target)
    return owner, slot, collected


class TerminalGateway:
    instance = 'test-instance'
    alive = True
    def __init__(self):
        self.calls = []
        self.lease = None
        self.fail_detach = False
    async def op(self,target,command,operation_id,clock):
        self.calls.append(command['kind'])
        if command['kind'] == 'open':
            self.lease = WriterLease(target['lock_path'])
            return {'opened':True,'instance':self.instance,'key':target['key'],'generation':target['generation'],
                'state':{'key':target['key'],'generation':target['generation'],'revision':0,
                    'view':{'terminal':True,'conversation':target['meta']['session_id']},'channels':{},'error':None}}
        if command['kind'] == 'close':
            if self.fail_detach:
                raise TimeoutError('silent Rust')
            self.lease.close()
            self.lease = None
            return {'closed':True}
        return {'operation_id':operation_id,'disposition':'accepted','payload':{}}


def test_terminal_prepare_adopt_without_cano(monkeypatch, tmp_path):
    gateway = TerminalGateway()
    owner, slot, _ = live_owner(monkeypatch,tmp_path,gateway=gateway)
    async def flow():
        assert await owner.prepare_session('session','claude')
        assert slot.phase == Phase.Rust
        assert gateway.calls == ['open']
        assert not slot.binding.headless and 'cano' not in slot.binding.meta
        assert ra.native_slot('session') is None
        await owner.detach('session')
    asyncio.run(flow())


def test_python_terminal_journal_dispatch_and_replay(monkeypatch,tmp_path):
    from app import terminal_input as ti
    owner, slot, _ = live_owner(monkeypatch,tmp_path)
    effects = []
    def submit(name,text):
        op = slot.store.state['operations']['root']
        assert op['status'] == 'dispatching' and op['dispatch_cursor']['absent_since']
        assert op['payload']['payload']['_terminal_generation'] == 1
        effects.append(text)
        return 'sent'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',lambda self,name,text:submit(name,text))
    async def flow():
        first = await owner.op('session',{'kind':'submit','text':'Olá 😀'},'root')
        assert first['disposition'] == 'accepted'
        assert await owner.op('session',{'kind':'submit','text':'Olá 😀'},'root') == first
        assert effects == ['Olá 😀']
        assert slot.store.state['operations']['root']['entry_materialized']
        assert slot.store.state['rows'][0]['delivered'] and not slot.store.state['rows'][0].get('confirmed')
    asyncio.run(flow())


@pytest.mark.parametrize('stage,status', [('linha.prova','deferred'),('linha.submeter','unknown')])
def test_partial_before_and_after_enter_different_recovery(monkeypatch,tmp_path,stage,status):
    from app import terminal_input as ti
    owner,slot,_ = live_owner(monkeypatch,tmp_path)
    def partial(self,name,text):
        ti._ULTIMA_LIMPEZA.stage=stage
        ti._ULTIMA_LIMPEZA.limpou=True
        return 'partial'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',partial)
    async def flow():
        result = await owner.op('session',{'kind':'submit','text':'partial'},'root')
        assert result['disposition'] == status
        row = slot.store.state['rows'][0]
        assert row.get('attempts',0) == (1 if status == 'deferred' else 0)
        assert row['delivered'] == (status == 'unknown')
        if status == 'deferred':
            assert slot.store.state['operations']['root']['terminal_finalized']
    asyncio.run(flow())


def test_slash_without_row_clear_blocks_until_generation_change(monkeypatch,tmp_path):
    from app import terminal_input as ti
    owner,slot,collected = live_owner(monkeypatch,tmp_path)
    calls=[]
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',lambda self,name,text:calls.append(text) or 'sent')
    async def flow():
        assert (await owner.op('session',{'kind':'submit','text':'/clear'},'clear'))['disposition'] == 'accepted'
        assert slot.store.state['rows'] == []
        assert slot.store.state['runtime_state']['clear_barrier']['generation'] == 1
        with pytest.raises(RuntimeError): await owner.op('session',{'kind':'submit','text':'blocked'},'blocked')
        collected['session_id']='after'
        collected['jsonl']=str(tmp_path / 'after.jsonl')
        assert await owner.prepare_session('session','claude')
        assert slot.binding.generation == 2 and slot.binding.meta['terminal']['generation'] == 2
        assert (await owner.op('session',{'kind':'submit','text':'new'},'new'))['disposition']=='accepted'
        assert calls == ['/clear','new']
    asyncio.run(flow())


def test_clear_unproved_before_enter_raises_no_barrier(monkeypatch,tmp_path):
    # #84: o texto do /clear não chegou ao composer e o Enter nunca saiu: entrega incerta comum.
    from app import terminal_input as ti
    owner,slot,_ = live_owner(monkeypatch,tmp_path)
    calls=[]
    def send(self,name,text):
        calls.append(text)
        if text == '/clear':
            ti._ULTIMA_LIMPEZA.stage='linha.prova'
            ti._ULTIMA_LIMPEZA.limpou=False
            return 'partial'
        return 'sent'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',send)
    async def flow():
        assert (await owner.op('session',{'kind':'submit','text':'/clear'},'clear'))['disposition'] == 'unknown'
        assert 'clear_barrier' not in slot.store.state['runtime_state']
        assert (await owner.op('session',{'kind':'submit','text':'new'},'new'))['disposition'] == 'accepted'
        assert calls == ['/clear','new']
    asyncio.run(flow())


def test_clear_without_new_conversation_releases_barrier_without_resending(monkeypatch,tmp_path):
    from app import runtime_terminal as rt, terminal_input as ti
    owner,slot,_ = live_owner(monkeypatch,tmp_path)
    calls=[]
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',lambda self,name,text:calls.append(text) or 'sent')
    async def flow():
        assert (await owner.op('session',{'kind':'submit','text':'/clear'},'clear'))['disposition'] == 'accepted'
        with pytest.raises(RuntimeError): await owner.op('session',{'kind':'submit','text':'blocked'},'blocked')
        # Passado o prazo, sem conversa nova no vínculo nem no disco: a trava sai.
        monkeypatch.setattr(rt,'CLEAR_APPLY_WAIT_S',0)
        assert (await owner.op('session',{'kind':'submit','text':'new'},'new'))['disposition'] == 'accepted'
        assert 'clear_barrier' not in slot.store.state['runtime_state']
        op = slot.store.state['operations']['clear']
        assert op['status'] == 'rejected' and op['result']['payload']['code'] == 'clear_not_applied'
        assert calls == ['/clear','new']
    asyncio.run(flow())


def test_same_name_new_mux_does_not_import_old_queue(monkeypatch,tmp_path):
    owner,slot,collected=live_owner(monkeypatch,tmp_path)
    slot.store.exec(1,'append',{'monotonic_s':1,'epoch_s':10},
        {'kind':'append','text':'old debt','delivered':False,'ts':10,'pre_transcript':False,'entry_id':'old'})
    collected['namespace']='socket:other-pid:other-birth'
    async def flow():
        assert await owner.prepare_session('session','claude')
        new = owner.slot('session')
        assert new.binding.key != slot.binding.key
        assert new.store.state['rows']==[]
        assert slot.store.state['rows'][0]['text']=='old debt'
        assert slot.phase == Phase.RecoveringPython and slot.lease is None
    asyncio.run(flow())


def test_plugin_click_writes_with_the_terminal_ownership(monkeypatch,tmp_path):
    # O clique de mod pelo app com o Rust dono do pane: sem a posse, era o 500 do log de 04/10/2026.
    from app import plugin_click, tmux
    gateway=LoanGateway()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    sent=[]
    def send_keys(name, keys, literal=False):
        from app.runtime_terminal import assert_writer
        assert_writer(name)
        sent.append((name, keys, slot.phase))
        return True
    monkeypatch.setattr(tmux,'send_keys',send_keys)
    async def flow():
        await _to_rust(owner,'session')
        assert await asyncio.to_thread(plugin_click.click,'session',2,5)
        assert [(name, phase) for name, _, phase in sent]==[('session',Phase.Rust)]
        assert [kind for kind,_ in gateway.controls] == ['keyboard_loan','keyboard_return']
        await owner.detach('session')
    asyncio.run(flow())


def test_admin_loan_refused_has_zero_effect(monkeypatch,tmp_path):
    from app.runtime_terminal import run_admin
    class Silent(LoanGateway):
        async def op(self,target,command,operation_id,clock):
            if command['kind'] == 'control' and command['control'] == 'keyboard_loan':
                raise TimeoutError('silent Rust')
            return await super().op(target,command,operation_id,clock)
    gateway=Silent()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    calls=[]
    async def flow():
        await _to_rust(owner,'session')
        with pytest.raises(TimeoutError):
            await run_admin(owner,'session','permission',{},lambda:calls.append('BTab'))
        assert calls==[] and slot.phase == Phase.Rust
        gateway.lease.close()
    asyncio.run(flow())


def test_admin_cancel_waits_thread_and_returns_keyboard(monkeypatch,tmp_path):
    from app.runtime_terminal import run_admin
    gateway=LoanGateway()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    entered,release=threading.Event(),threading.Event()
    def action():
        entered.set()
        assert slot.phase == Phase.Rust and slot.store is None
        release.wait(3)
        return 'mode'
    async def flow():
        await _to_rust(owner,'session')
        task=asyncio.create_task(run_admin(owner,'session','permission',{},action))
        while not entered.is_set(): await asyncio.sleep(.001)
        task.cancel()
        await asyncio.sleep(.01)
        assert not task.done(), 'o cancelamento espera a digitação em curso'
        release.set()
        with pytest.raises(asyncio.CancelledError): await task
        assert [kind for kind,_ in gateway.controls] == ['keyboard_loan','keyboard_return']
        assert slot.phase == Phase.Rust
        await owner.detach('session')
    asyncio.run(flow())


def test_generation_changed_during_serial_wait_has_zero_effect(monkeypatch,tmp_path):
    from app import terminal_input as ti
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    calls=[]
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',lambda self,name,text:calls.append(text) or 'sent')
    async def flow():
        await slot.terminal_serial.acquire()
        task=asyncio.create_task(owner.op('session',{'kind':'submit','text':'stale'},'stale'))
        while slot.active==0: await asyncio.sleep(0)
        slot.binding.generation=2
        slot.terminal_serial.release()
        with pytest.raises(RuntimeError): await task
        assert calls==[]
    asyncio.run(flow())


def test_native_receipt_for_appended_entry_without_root(monkeypatch,tmp_path):
    import uuid
    from app import terminal_input as ti, runtime_terminal as terminal
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    sender='uds:fake'
    mid=str(uuid.uuid5(uuid.NAMESPACE_URL,f'hangar:{slot.binding.key}:entry'))
    real_service=terminal._service
    def service(coordinator,descriptor,operation,request,kind,payload):
        if kind=='terminal_facts':
            return dict(binding=payload['binding'],ready=True,idle=True,open_question=False,plugin_live=False,
                plugin_user=False,clipboard_available=False,native={'message_id':mid})
        return real_service(coordinator,descriptor,operation,request,kind,payload)
    monkeypatch.setattr(terminal,'_service',service)
    monkeypatch.setattr(terminal,'_native_send',lambda *args:'written')
    async def flow():
        slot.store.exec(1,'append',{'monotonic_s':1,'epoch_s':10},
            {'kind':'append','text':'[de: sender] hi','delivered':False,'ts':10,'pre_transcript':False,'entry_id':'entry'})
        assert (await owner.op('session',{'kind':'drain'},'drain'))['sent']==1
        assert 'entry' not in slot.store.state['operations']
        assert await owner.native_receipt(mid,'refused')
        assert slot.store.state['rows'][0]['desistiu']
        assert not slot.store.state['rows'][0].get('confirmed')
    asyncio.run(flow())


def test_rust_terminal_drain_shape_normalized_for_existing_callers(monkeypatch,tmp_path):
    gateway=TerminalGateway()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    original=gateway.op
    async def rpc(target,command,operation_id,clock):
        if command['kind']=='drain':
            return {'drained':1,'reply':{'operation_id':'queued','disposition':'accepted','payload':{}}}
        return await original(target,command,operation_id,clock)
    gateway.op=rpc
    async def flow():
        await _to_rust(owner,'session')
        assert (await owner.op('session',{'kind':'drain'},'drain'))['sent']==1
        await owner.detach('session')
    asyncio.run(flow())


def test_retire_old_rust_life_before_new_name_binding(monkeypatch,tmp_path):
    gateway=TerminalGateway()
    owner,slot,collected=live_owner(monkeypatch,tmp_path,gateway=gateway)
    async def flow():
        await _to_rust(owner,'session')
        collected['namespace']='socket:restarted:birth'
        assert await owner.prepare_session('session','claude')
        assert owner.slot('session') is not slot
        assert slot.lease is None and slot.phase==Phase.RecoveringPython
        # O freeze da retirada fecha a porta antes do `close` e a reabre antes da nova vida abrir.
        assert gateway.calls==['open','ingress','close','ingress','open']
        await owner.detach('session')
    asyncio.run(flow())


def test_native_transport_allowed_while_composer_not_ready(monkeypatch,tmp_path):
    from app import runtime_terminal as terminal
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    effects=[]
    def service(coordinator,descriptor,operation,request,kind,payload):
        return dict(binding=payload['binding'],ready=False,idle=False,open_question=True,plugin_live=False,
            plugin_user=False,clipboard_available=False,native={'message_id':'native'})
    monkeypatch.setattr(terminal,'_service',service)
    monkeypatch.setattr(terminal,'_native_send',lambda *args:effects.append('socket') or 'written')
    async def flow():
        result=await owner.op('session',{'kind':'submit','text':'[de: sender] hello'},'root')
        assert result['disposition']=='accepted' and effects==['socket']
    asyncio.run(flow())


def test_human_clear_refreshed_before_direct_drain(monkeypatch,tmp_path):
    owner,slot,collected=live_owner(monkeypatch,tmp_path)
    async def flow():
        collected['jsonl']=str(tmp_path / 'new.jsonl')
        collected['session_id']='new'
        await owner.op('session',{'kind':'drain'},'drain')
        assert slot.binding.generation==2
    asyncio.run(flow())


def test_terminal_facts_fresh_identity_before_plugin_effect(monkeypatch,tmp_path):
    from app import runtime_terminal as terminal
    owner,slot,collected=live_owner(monkeypatch,tmp_path)
    descriptor=slot.binding.descriptor()
    collected['namespace']='socket:restarted:birth'
    metadata={**descriptor['meta'],'descriptor':descriptor,'operation_id':'root','validate':lambda:None}
    with pytest.raises(RuntimeError):
        terminal.publish({'binding':descriptor['meta']['terminal'],'generation':1,
            'publication':{'id':'pub','text':'text','mode':'fill'}},metadata)


def test_clipboard_lease_reentrant_and_excludes_other_process(monkeypatch,tmp_path):
    from app import runtime_terminal as terminal
    import subprocess,sys
    path=tmp_path/'clipboard.lock'
    monkeypatch.setattr(terminal,'_clipboard_lock_path',lambda:path)
    lock=terminal.ClipboardLock()
    script="import sys; from app.runtime_coordinator import WriterLease\ntry:\n WriterLease(sys.argv[1])\nexcept BlockingIOError:\n sys.exit(0)\nsys.exit(1)"
    with lock:
        with lock:
            assert subprocess.run([sys.executable,'-c',script,str(path)],timeout=5).returncode==0
        assert subprocess.run([sys.executable,'-c',script,str(path)],timeout=5).returncode==0
    lease=WriterLease(path)
    lease.close()


def test_reserve_consumes_partial_proof_on_reused_worker(monkeypatch,tmp_path):
    from app import terminal_input as ti
    from concurrent.futures import ThreadPoolExecutor
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    def partial(self,name,text):
        ti._ULTIMA_LIMPEZA.stage='linha.prova'
        ti._ULTIMA_LIMPEZA.limpou=True
        return 'partial'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',partial)
    async def flow():
        asyncio.get_running_loop().set_default_executor(ThreadPoolExecutor(max_workers=1))
        result=await owner.op('session',{'kind':'submit','text':'partial'},'root')
        assert result['disposition']=='deferred'
        assert not await asyncio.to_thread(lambda:hasattr(ti._ULTIMA_LIMPEZA,'limpou'))
        assert not await asyncio.to_thread(lambda:hasattr(ti._ULTIMA_LIMPEZA,'stage'))
    asyncio.run(flow())


@pytest.mark.parametrize('ack', ['matching','old','wrong_generation','wrong_conversation','uncorrelated'])
def test_managed_held_permission_ack_correlates_without_fallback(monkeypatch,ack):
    monkeypatch.setattr(pb,'CONFIRMA_S',.05)
    async def flow():
        pb._loop=asyncio.get_running_loop()
        queue=asyncio.Queue()
        pb._perguntas['session']={'id':'perm:request','questions':[], 'fila':queue,'visto':time.monotonic()}
        receipt={'publication_id':'new','generation':1,'session_id':'sid'}
        task=asyncio.create_task(asyncio.to_thread(pb.responder_pergunta,'session',{'permitir':True},'perm:request',receipt))
        body=await asyncio.wait_for(queue.get(),.5)
        assert body['publication_id']=='new'
        extra=copy.deepcopy(receipt)
        if ack=='old':extra['publication_id']='old'
        if ack=='wrong_generation':extra['generation']=0
        if ack=='wrong_conversation':extra['session_id']='old-sid'
        if ack=='uncorrelated':extra={}
        await pb.ask_fim(pb.AskFimBody(sessao='session',token=pb.mint('session'),id='perm:request',vencedor='app',**extra))
        assert await task == (ack=='matching')
    try:
        asyncio.run(flow())
    finally:
        pb._perguntas.clear()
        pb._fechadas.clear()


@pytest.mark.skipif(sys.platform == 'win32', reason='CLI falsa POSIX; Windows tem Job real separado')
def test_binding_and_reserve_real_fake_cli_isolated_mux(monkeypatch,tmp_path):
    import os, shlex, subprocess, sys, uuid
    from app import api, pqueue, tmux, runtime_terminal as terminal
    from app.registry import SessionRegistry, sanitize_cwd
    from app import procinfo, registry as registry_module
    from app.runtime_adapter import LegacyBridge
    label='hangar-task3-'+str(os.getpid())+'-'+uuid.uuid4().hex
    sid=str(uuid.uuid4())
    config=tmp_path/'config'
    transcript=config/'projects'/sanitize_cwd(str(tmp_path))/(sid+'.jsonl')
    transcript.parent.mkdir(parents=True)
    cli=tmp_path/'fake_cli.py'
    cli.write_text('''import os,sys,tty,codecs,json,datetime
fd=sys.stdin.fileno(); tty.setraw(fd); text=''; decoder=codecs.getincrementaldecoder('utf-8')()
def render():
 sys.stdout.write('\\x1b[2J\\x1b[Hhistory\\r\\n'+'─'*40+'\\r\\n❯ '+text+'\\r\\n'+'─'*40+'\\r\\n? for shortcuts\\r\\n');sys.stdout.flush()
render()
while True:
 for c in decoder.decode(os.read(fd,4096)):
  if c=='\\r':
   event={'type':'user','sessionId':sys.argv[-2],'uuid':str(datetime.datetime.now().timestamp()),'timestamp':datetime.datetime.now(datetime.timezone.utc).isoformat(),'message':{'role':'user','content':text}}
   with open(sys.argv[-1],'a',encoding='utf-8') as f:f.write(json.dumps(event,ensure_ascii=False)+'\\n')
   text=''
  elif c=='\\x15':text=''
  else:text+=c
  render()
''')
    env={key:value for key,value in os.environ.items() if key not in {'CP_AUTH_TOKEN','HANGAR_INTERNAL_SECRET','HANGAR_RUNTIME_INSTANCE'}}
    env['CLAUDE_CONFIG_DIR']=str(config)
    command=shlex.join(['env','CLAUDE_CONFIG_DIR='+str(config),sys.executable,str(cli),'--session-id',sid,str(transcript)])
    subprocess.run(['tmux','-L',label,'-f','/dev/null','new-session','-d','-s','session','-x','100','-y','40','-c',str(tmp_path),
        shlex.join(['bash','-c',command])],env=env,check=True)
    raw_run=subprocess.run
    def mux_run(args,**kwargs):
        return raw_run(args[:1]+['-L',label]+args[1:] if args[0]=='tmux' else args,**kwargs)
    monkeypatch.setattr(tmux,'RUN',mux_run)
    identify=registry_module._provider_do_argv
    monkeypatch.setattr(registry_module,'_provider_do_argv',
        lambda argv:'claude' if str(cli) in argv else identify(argv))
    monkeypatch.setattr(api,'registry',SessionRegistry(projects_dir=config/'projects'))
    monkeypatch.setattr(pqueue,'_queue_dir',lambda:tmp_path/'queues')
    owner=RuntimeCoordinator()
    owner.legacy=LegacyBridge(owner,{'claude':object()})
    try:
        deadline=time.monotonic()+3
        binding=None
        while time.monotonic()<deadline:
            binding=terminal.resolve_binding('session')
            if binding and '❯' in tmux.capture_pane('session'): break
            time.sleep(.01)
        pane=tmux.list_panes_all()['session'][0]
        agent=registry_module.agente_do_pane(pane['pid'])[1]
        if binding is None:
            pytest.fail(json.dumps({'agent':agent, 'argv':procinfo._argv(pane['pid']),
                'screen':tmux._run(['tmux','capture-pane','-p','-t',pane['pane_id']]).stdout[-800:]}))
        assert binding.jsonl==str(transcript) and not transcript.exists()
        slot=owner.register(binding)
        async def flow():
            result=await owner.op('session',{'kind':'submit','text':'ação 😀 C:\\Users\\test'},'real-io')
            assert result['disposition']=='accepted'
            assert await owner.op('session',{'kind':'submit','text':'ação 😀 C:\\Users\\test'},'real-io')==result
            await owner.op('session',{'kind':'confirm'},'confirm')
            assert slot.store.state['rows'][0]['confirmed']
        asyncio.run(flow())
        rows=[json.loads(row) for row in transcript.read_text().splitlines()]
        assert len(rows)==1 and rows[0]['message']['content']=='ação 😀 C:\\Users\\test'
    finally:
        owner.close_python_leases()
        raw_run(['tmux','-L',label,'kill-server'],capture_output=True)


def test_first_legacy_queue_import_uses_precise_birth_and_keeps_original(monkeypatch,tmp_path):
    from app import runtime_terminal as terminal
    owner,old,collected=live_owner(monkeypatch,tmp_path)
    owner.close_python_leases()
    collected['pane_birth']=1.5
    projection=tmp_path/'session.jsonl'
    rows=[{'id':'old','text':'old life','ts':1.2,'delivered':False},
        {'id':'current','text':'current life','ts':1.7,'delivered':False}]
    projection.write_text(''.join(json.dumps(row)+'\n' for row in rows))
    # A fila é da migração inicial; nenhum diário anterior de outra chave com esse nome.
    old.binding.state_path.unlink()
    owner=RuntimeCoordinator()
    binding=terminal.resolve_binding('session')
    slot=owner.register(binding)
    assert [row['id'] for row in slot.store.state['rows']]==['current']
    backups=list(binding.state_path.parent.glob('*.legacy.jsonl'))
    assert len(backups)==1 and [json.loads(row) for row in backups[0].read_text().splitlines()]==rows


def test_reserve_driver_unknown_never_exposes_driveerror_for_text_fallback(monkeypatch,tmp_path):
    from app import terminal_input as ti
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    monkeypatch.setattr(ti.TerminalInput,'select',lambda *args,**kwargs:(_ for _ in ()).throw(ti.DriveError('uncertain')))
    async def flow():
        with pytest.raises(RuntimeError) as error:
            await owner.op('session',{'kind':'control','control':'select','payload':{'option':1}},'select')
        assert not isinstance(error.value,ti.DriveError)
        assert slot.store.state['operations']['select']['status']=='unknown'
    asyncio.run(flow())


def test_new_writer_waits_descendant_cleanup_after_abrupt_rust_death(monkeypatch,tmp_path):
    import os,subprocess
    from types import SimpleNamespace
    from app.runtime_process import spawn_contained, cleanup
    from app import terminal_input as ti
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    marker=tmp_path/'child.pid'
    script="import subprocess,sys,time; child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)']); open(sys.argv[1],'w').write(str(child.pid)); time.sleep(60)"
    proc=spawn_contained([sys.executable,'-c',script,str(marker)],env=dict(os.environ))
    proof=SimpleNamespace(containment_clean=lambda:proc.runtime_containment.cleaned)
    try:
        deadline=time.monotonic()+5
        while (not marker.exists() or not marker.read_text()) and time.monotonic()<deadline:time.sleep(.01)
        slot.lease.close()
        slot.lease=None
        slot.phase=Phase.Rust
        proc.kill()
        proc.wait(5)
        effects=[]
        monkeypatch.setattr(ti.TerminalInput,'interrupt',lambda *args,**kw:effects.append('new-writer'))
        async def flow():
            with pytest.raises(RuntimeError):await owner.recover('session',confirmed_dead=True,containment=proof)
            assert slot.lease is None and effects==[]
            assert await asyncio.to_thread(cleanup,proc)
            await owner.recover('session',confirmed_dead=True,containment=proof)
            assert slot.phase==Phase.Python
            await owner.op('session',{'kind':'control','control':'interrupt','payload':{}},'new')
            assert effects==['new-writer']
        asyncio.run(flow())
    finally:
        cleanup(proc)


def test_python_terminal_public_queue_cannot_confirm_by_text(monkeypatch,tmp_path):
    from app.pqueue import PromptQueue
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    async def flow():
        owner.loop=asyncio.get_running_loop()
        await asyncio.to_thread(PromptQueue('session').append,'message')
        before=copy.deepcopy(slot.store.state['rows'])
        with pytest.raises(RuntimeError):
            await asyncio.to_thread(PromptQueue('session').confirm_delivered)
        assert slot.store.state['rows']==before
    asyncio.run(flow())


def test_restart_validates_terminal_life_and_advances_clear(monkeypatch,tmp_path):
    from app.runtime_adapter import LegacyBridge
    from app.adapters.claude_headless import sessions
    from app.adapters.codex import sessions as codex_sessions
    owner,slot,collected=live_owner(monkeypatch,tmp_path)
    owner.close_python_leases()
    collected['session_id']='after'
    collected['jsonl']=str(tmp_path/'after.jsonl')
    monkeypatch.setattr(sessions,'list_all',lambda:[])
    monkeypatch.setattr(codex_sessions,'list_all',lambda:[])
    restored=RuntimeCoordinator()
    async def flow():
        await restored.start_sessions({'claude':object(),'codex':object()})
        current=restored.slot('session')
        assert current.binding.key==slot.binding.key and current.binding.generation==2
        assert current.binding.meta['terminal']['conversation']=='after'
    try:
        asyncio.run(flow())
    finally:
        restored.close_python_leases()


def test_clear_serialized_and_clears_old_queue(monkeypatch,tmp_path):
    from app import terminal_input as ti
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    slot.store.exec(1,'append',{'monotonic_s':1,'epoch_s':10},
        {'kind':'append','text':'old','delivered':False,'ts':10,'pre_transcript':False,'entry_id':'old'})
    def clear(self,name,text):
        assert slot.frozen
        return 'sent'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',clear)
    async def flow():
        await owner.op('session',{'kind':'submit','text':'/clear'},'clear')
        assert slot.store.state['rows']==[]
        assert slot.store.state['runtime_state']['clear_barrier']['generation']==1
    asyncio.run(flow())


@pytest.mark.parametrize('kind', ['option','text','chat'])
def test_answer_model_dump_reaches_terminal_contract(monkeypatch,tmp_path,kind):
    from app import api
    from app.runtime_terminal import answer_sync
    gateway=TerminalGateway()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    item=api.AnswerItem(kind=kind,question_id='question-a',indices=[0] if kind=='option' else None,
        labels=['A'] if kind=='option' else [],value='texto' if kind=='text' else None,
        type_index=1 if kind=='text' else None,chat_index=1 if kind=='chat' else None)
    monkeypatch.setattr(pb,'pergunta_pendente',lambda name:{'id':'ask:tui-request','questions':[{'id':'question-a'}]})
    observed=[]
    original=gateway.op
    async def op(target,command,operation,clock):
        if command['kind']=='control':
            observed.append(command['payload'])
        return await original(target,command,operation,clock)
    gateway.op=op
    async def flow():
        await _to_rust(owner,'session')
        # Id explícito de pergunta TUI evita o caminho composto para chat sem pedido.
        result=await asyncio.to_thread(answer_sync,'session',[item.model_dump()],'tui-request',None)
        assert result['disposition']=='accepted'
        assert observed[0]['answers'][0]['question_id']=='question-a'
        assert observed[0]['answers'][0]['indices']==([0] if kind=='option' else [])
        await owner.detach('session')
    asyncio.run(flow())


def test_explicit_claude_steer_uses_owner_and_transcript_confirmation(monkeypatch,tmp_path):
    from app import api
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=TerminalGateway())
    monkeypatch.setattr(api,'_provider_of',lambda name:'claude')
    monkeypatch.setattr(api,'_headless',lambda name:False)
    monkeypatch.setattr(api,'_session_exists',lambda name:True)
    monkeypatch.setattr(api,'_pane_info',lambda name:('claude','%1'))
    monkeypatch.setattr(api.PromptQueue,'confirm_delivered',lambda *args:pytest.fail('confirmação por ausência de prova'))
    async def flow():
        await _to_rust(owner,'session')
        result=await api.steer_session('session')
        assert result['promoted'] and result['confirmed']==0
        assert owner.transport.calls[-2:]==['control','confirm']
        await owner.detach('session')
    asyncio.run(flow())


@pytest.mark.parametrize('text', ['bad\x7f','bad\u0085'])
def test_terminal_unicode_controls_rejected_before_intent(monkeypatch,tmp_path,text):
    from app import terminal_input as ti
    owner,slot,_=live_owner(monkeypatch,tmp_path)
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',lambda *args,**kwargs:pytest.fail('driver antes da validação'))
    before=copy.deepcopy(slot.store.state)
    async def flow():
        with pytest.raises(ValueError):await owner.op('session',{'kind':'submit','text':text},'invalid')
        assert slot.store.state==before
    asyncio.run(flow())


@pytest.mark.parametrize('source', ['input','shared_send','send_one','hook','timer','adapter_send','adapter_drain'])
def test_claude_terminal_all_producers_route_to_owner(monkeypatch,tmp_path,source):
    from app import api
    from app.adapters.claude import ClaudeAdapter
    from types import SimpleNamespace
    gateway=TerminalGateway()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    monkeypatch.setattr(api,'_provider_of',lambda name:'claude')
    monkeypatch.setattr(api,'_headless',lambda name:False)
    monkeypatch.setattr(api,'_session_exists',lambda name:True)
    monkeypatch.setattr(api,'_recusa_orq',lambda name:None)
    monkeypatch.setattr(api,'_cached_info_sync',lambda name:SimpleNamespace(jsonl=slot.binding.jsonl,provider='claude'))
    async def flow():
        await _to_rust(owner,'session')
        if source=='input':await api.input_prompt('session',api.InputBody(text='text',steer=True))
        elif source=='shared_send':await api._enviar('session','[grupo: sender] message')
        elif source=='send_one':await asyncio.to_thread(api._send_one,'session','text')
        elif source=='hook':await asyncio.to_thread(api._drenar,'session',slot.binding.jsonl,'claude')
        elif source=='timer':await asyncio.to_thread(api._confirm_and_drain,'session')
        elif source=='adapter_send':await ClaudeAdapter().send_prompt('session','text')
        else:await ClaudeAdapter().drain('session',slot.binding.jsonl)
        calls=gateway.calls[1:]
        assert calls == (['confirm','drain'] if source=='timer' else ['drain'] if source in {'hook','adapter_drain'} else ['submit'])
        await owner.detach('session')
    asyncio.run(flow())


@pytest.mark.parametrize('control', ['send_key','send_term_key','send_text','select','submeter_multipla','interrupt','answer_questions','steer_now'])
def test_claude_terminal_controls_route_to_owner(monkeypatch,tmp_path,control):
    from app import terminal_input as ti
    gateway=TerminalGateway()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    async def flow():
        await _to_rust(owner,'session')
        terminal=ti.TerminalInput()
        if control in {'send_key','send_term_key'}:await asyncio.to_thread(getattr(terminal,control),'session','Up')
        elif control=='send_text':await asyncio.to_thread(terminal.send_text,'session','text')
        elif control=='select':await asyncio.to_thread(terminal.select,'session',1)
        elif control in {'submeter_multipla','interrupt'}:await asyncio.to_thread(getattr(terminal,control),'session')
        elif control=='steer_now':await asyncio.to_thread(ti.steer_now,'session','claude')
        else:await asyncio.to_thread(ti.answer_questions,'session',[{'kind':'option','indices':[0],'labels':['A']}])
        assert gateway.calls==['open','control']
        await owner.detach('session')
    asyncio.run(flow())


@pytest.mark.parametrize('kind,status', [('share',200),('pair',403),('guest_user',200)])
def test_public_guest_routes_follow_owner_and_readonly_pair_zero_effect(monkeypatch,tmp_path,kind,status):
    from app import api, share_store,share_gate,guest_users
    from app.config import settings
    from fastapi.testclient import TestClient
    gateway=TerminalGateway()
    owner,slot,_=live_owner(monkeypatch,tmp_path,gateway=gateway)
    monkeypatch.setattr(settings,'auth_token','owner')
    monkeypatch.setattr(api,'_provider_of',lambda name:'claude')
    monkeypatch.setattr(api,'_headless',lambda name:False)
    monkeypatch.setattr(api,'_session_exists',lambda name:True)
    monkeypatch.setattr(api,'_recusa_orq',lambda name:None)
    share=share_store.Share(id='test',session='session',life='life',created_at=0,code_expires_at=9e9,
        code_hash='',token_hash='token',redeemed_at=1,kind=kind)
    if kind=='guest_user':
        guest=guest_users.Guest('test','guest','hash',str(tmp_path))
        monkeypatch.setattr(guest_users,'lookup_token',lambda token:guest if token=='guest' else None)
        monkeypatch.setattr(guest_users,'visible_to',lambda guest,name:name=='session')
        client=TestClient(api.app,base_url='http://127.0.0.1:8765')
    else:
        monkeypatch.setattr(share_store,'lookup_token',lambda token:share_store.Guest([share]))
        monkeypatch.setattr(share_gate,'session_life',lambda name:'life')
        share_gate._life_cache.clear()
        client=TestClient(api.app,base_url='http://127.0.0.1:8766',client=('203.0.113.9',1))
    async def flow():
        await _to_rust(owner,'session')
        result=await asyncio.to_thread(client.post,'/api/sessions/session/input',
            json={'text':'text'},headers={'Authorization':'Bearer guest'})
        assert result.status_code==status
        assert gateway.calls==(['open','submit'] if status==200 else ['open'])
        await owner.detach('session')
    asyncio.run(flow())


def test_terminal_native_socket_belongs_to_current_agent_not_reused_sid(monkeypatch,tmp_path):
    from app import runtime_terminal as terminal
    from app.runtime_coordinator import Binding
    current=terminal_binding(tmp_path)
    current.meta['agent_pid']=42
    current.meta['config_dir']=str(tmp_path)
    directory=tmp_path/'sessions'
    directory.mkdir()
    own=tmp_path/'own.sock'
    other=tmp_path/'other.sock'
    own.touch();other.touch()
    (directory/'41.json').write_text(json.dumps({'pid':41,'sessionId':'sid','messagingSocketPath':str(other)}))
    (directory/'42.json').write_text(json.dumps({'pid':42,'sessionId':'sid','messagingSocketPath':str(own)}))
    assert terminal._native_socket(current)==str(own)


def test_plugin_instance_from_old_life_not_published(monkeypatch,tmp_path):
    from app import runtime_terminal as terminal
    current=terminal_binding(tmp_path)
    current.meta['created']=123.5
    pb._donos['session']=('1-test',{'receipt_v2','user'},time.monotonic())
    monkeypatch.setattr(terminal,'validate_binding',lambda descriptor:current)
    monkeypatch.setattr(pb,'publish_terminal',lambda *args:pytest.fail('publicação na vida antiga'))
    try:
        assert terminal.publish({'binding':current.meta['terminal'],'generation':1,
            'publication':{'id':'new','text':'text','mode':'fill'}},
            {'descriptor':current.descriptor(),'validate':lambda:None})=='not_written'
    finally:
        pb._donos.clear()


def test_headless_to_terminal_pending_binding_keeps_key_and_old_queue(monkeypatch,tmp_path):
    from app import runtime_terminal as terminal, terminal_input as ti
    from app.runtime_adapter import LegacyBridge
    from app.adapters.claude_headless import sessions
    from app import pqueue
    original=terminal_binding(tmp_path)
    original.headless=True
    original.key='headless_key'
    original.meta={'key':original.key,'session_id':'sid','created':0}
    original.state_path=tmp_path/'runtime'/'headless_key.json'
    original.lock_path=tmp_path/'runtime'/'headless_key.lock'
    class Legacy:
        def binding(self,*args):return None
        async def quiesce(self,descriptor):return {}
    owner=RuntimeCoordinator(legacy=Legacy())
    slot=owner.register(original)
    slot.store.exec(1,'append',{'monotonic_s':1,'epoch_s':.5},
        {'kind':'append','text':'carry','delivered':False,'ts':.5,'pre_transcript':False,'entry_id':'carry'})
    monkeypatch.setattr(terminal,'_session_proof',lambda name:'session-life',raising=False)
    async def flow():
        async def switch():return None
        await owner.change('session',switch,reopen=False)
        assert slot.binding.meta['pending_terminal']=='session-life'
        assert owner.managed_runtime('session') and ra.native_slot('session') is None
        with pytest.raises(RuntimeError):terminal.assert_writer('session')
        collected=dict(name='session',pane='%1',created=1,namespace='mux-life',pane_birth=1.5,
            session_proof='session-life',jsonl=original.jsonl,session_id='sid',config_dir=str(tmp_path),cwd=str(tmp_path),mux_argv=['tmux'],windows=False)
        monkeypatch.setattr(terminal,'_collect',lambda name:collected)
        monkeypatch.setattr(sessions,'load',lambda name:None)
        monkeypatch.setattr(pqueue,'_queue_dir',lambda:tmp_path)
        monkeypatch.setattr(ti,'_capture',lambda name:'─'*30+'\n❯ \n'+'─'*30+'\n')
        monkeypatch.setattr(ti,'classify',lambda pane:('idle',None,None,None))
        monkeypatch.setattr(ti,'is_overlay',lambda pane:False)
        monkeypatch.setattr(pb,'pergunta_pendente',lambda name:None)
        monkeypatch.setattr(ti.TerminalInput,'send_prompt',lambda *args:'sent')
        owner.legacy=LegacyBridge(owner,{'claude':object()})
        assert await owner.prepare_session('session','claude')
        assert slot.binding.key=='headless_key' and slot.binding.generation==2
        assert slot.store.state['runtime_state']['_binding']['meta'].get('terminal')
        assert (await owner.op('session',{'kind':'drain'},'drain'))['sent']==1
    asyncio.run(flow())


def _reborn_terminal(monkeypatch, tmp_path):
    """Sessão com terminal cujo pane a ação mata e recria: o teste troca `life` para a vida nova."""
    from app import runtime_terminal as terminal, pqueue
    from app.runtime_adapter import LegacyBridge
    from app.adapters.claude_headless import sessions
    owner = RuntimeCoordinator()
    slot = owner.register(terminal_binding(tmp_path))
    slot.store.exec(1, 'append', {'monotonic_s': 1, 'epoch_s': .5},
        {'kind': 'append', 'text': 'carry', 'delivered': False, 'ts': .5, 'pre_transcript': False, 'entry_id': 'carry'})
    life = {'proof': 'old-life', 'facts': None}
    born = dict(name='session', pane='%7', created=456, namespace='mux-life', pane_birth=456.5,
        session_proof='new-life', jsonl=str(tmp_path / 'other-account' / 'sid.jsonl'), session_id='sid',
        config_dir=str(tmp_path / 'other-account'), cwd=str(tmp_path), mux_argv=['tmux'], windows=False,
        agent_pid=77, agent_birth=456.7)
    monkeypatch.setattr(terminal, '_session_proof', lambda name: life['proof'])
    monkeypatch.setattr(terminal, '_pane_life', lambda name: life.get('pane'), raising=False)
    monkeypatch.setattr(terminal, '_collect', lambda name: life['facts'])
    monkeypatch.setattr(sessions, 'load', lambda name: None)
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    owner.legacy = LegacyBridge(owner, {'claude': object()})
    return owner, slot, life, born


def test_account_move_reborn_terminal_keeps_key_and_queue(monkeypatch, tmp_path):
    owner, slot, life, born = _reborn_terminal(monkeypatch, tmp_path)
    async def flow():
        async def move():
            life.update(proof='new-life', facts=born)
        await owner.change('session', move, reopen=False)
        assert slot.binding.key == 'terminal_key' and slot.binding.generation == 2
        assert slot.binding.jsonl == born['jsonl'] and slot.binding.meta['terminal']['pane'] == '%7'
        assert not slot.binding.meta.get('pending_terminal')
        assert [row['text'] for row in slot.store.state['rows']] == ['carry']
    asyncio.run(flow())


def test_account_move_waits_for_agent_of_reborn_terminal(monkeypatch, tmp_path):
    from app import runtime_terminal as terminal
    owner, slot, life, born = _reborn_terminal(monkeypatch, tmp_path)
    async def flow():
        async def move():
            life['proof'] = 'new-life'
        await owner.change('session', move, reopen=False)
        assert slot.binding.key == 'terminal_key'
        assert slot.binding.meta['pending_terminal'] == 'new-life' and 'terminal' not in slot.binding.meta
        with pytest.raises(RuntimeError):
            terminal.assert_writer('session')
        life['facts'] = born
        assert await owner.prepare_session('session', 'claude')
        assert slot.binding.key == 'terminal_key' and slot.binding.jsonl == born['jsonl']
        assert slot.binding.meta['terminal']['pane'] == '%7' and not slot.binding.meta.get('pending_terminal')
        assert [row['text'] for row in slot.store.state['rows']] == ['carry']
    asyncio.run(flow())


def test_pending_terminal_reborn_again_keeps_key(monkeypatch, tmp_path):
    owner, slot, life, born = _reborn_terminal(monkeypatch, tmp_path)
    async def flow():
        async def move():
            life['proof'] = 'new-life'
        await owner.change('session', move, reopen=False)
        async def move_again():
            life.update(proof='newer-life', facts={**born, 'session_proof': 'newer-life', 'pane': '%9'})
        await owner.change('session', move_again, reopen=False)
        assert slot.binding.key == 'terminal_key' and slot.binding.meta['terminal']['pane'] == '%9'
        assert not slot.binding.meta.get('pending_terminal')
    asyncio.run(flow())


# --- Riscos anotados no PR #43 e a troca pelo caminho Rust (dono único, Task 4) ---

def test_reborn_terminal_with_other_conversation_does_not_inherit_key(monkeypatch, tmp_path):
    owner, slot, life, born = _reborn_terminal(monkeypatch, tmp_path)
    async def flow():
        async def move():
            life.update(proof='new-life', facts={**born, 'session_id': 'outra',
                'jsonl': str(tmp_path / 'other-account' / 'outra.jsonl')})
        await owner.change('session', move, reopen=False)
        current = owner.slot('session')
        assert current.binding.key != 'terminal_key' and current.binding.meta['session_id'] == 'outra'
        assert current.store.state['rows'] == [], 'a fila da conversa antiga não vai para a nova'
        assert 'terminal_key' not in owner.slots
    asyncio.run(flow())


def test_session_proof_survives_tmux_server_gone(monkeypatch):
    from types import SimpleNamespace
    from app import runtime_terminal as terminal, tmux
    # O servidor tmux morreu entre o display-message e a leitura do processo dele.
    monkeypatch.setattr(tmux, '_run', lambda args: SimpleNamespace(returncode=0,
        stdout='/tmp/sock\t999999999\t5\tsession\t$1\t999999998\n'))
    assert terminal._session_proof('session') is None
    assert terminal._pane_life('session') is None
    assert terminal.terminal_life(terminal_binding(Path('/tmp'))) is None


def test_respawn_pane_in_same_tmux_session_keeps_key(monkeypatch, tmp_path):
    owner, slot, life, born = _reborn_terminal(monkeypatch, tmp_path)
    life['pane'] = 'pane-antigo'
    async def flow():
        async def respawn():
            # `respawn-pane` na mesma sessão tmux: a prova da sessão fica, o pane é outro.
            life.update(pane='pane-novo', facts={**born, 'session_proof': 'old-life'})
        await owner.change('session', respawn, reopen=False)
        assert slot.binding.key == 'terminal_key' and slot.binding.meta['terminal']['pane'] == '%7'
        assert [row['text'] for row in slot.store.state['rows']] == ['carry']
    asyncio.run(flow())


def test_rust_account_move_reborn_terminal_reopens_with_key(monkeypatch, tmp_path):
    from app import runtime_terminal as terminal
    gateway = TerminalGateway()
    owner, slot, collected = live_owner(monkeypatch, tmp_path, gateway=gateway)
    facts = {'now': {**collected, 'session_proof': 'p1'}}
    proof = {'now': 'p1'}
    monkeypatch.setattr(terminal, '_collect', lambda name: {**facts['now'], 'name': name})
    monkeypatch.setattr(terminal, '_session_proof', lambda name: proof['now'])
    monkeypatch.setattr(terminal, '_pane_life', lambda name: None, raising=False)
    slot.store.exec(1, 'append', {'monotonic_s': 1, 'epoch_s': .5},
        {'kind': 'append', 'text': 'carry', 'delivered': False, 'ts': .5, 'pre_transcript': False, 'entry_id': 'carry'})
    async def flow():
        assert await owner.prepare_session('session', 'claude')
        assert slot.phase == Phase.Rust
        key = slot.binding.key
        async def move():
            assert gateway.calls[-1] == 'close' and gateway.lease is None
            proof['now'] = 'p2'
            facts['now'] = {**collected, 'pane': '%7', 'created': 456, 'namespace': 'mux-novo', 'session_proof': 'p2',
                'jsonl': str(tmp_path / 'outra-conta' / 'sid.jsonl'), 'config_dir': str(tmp_path / 'outra-conta')}
        await owner.change('session', move)
        # A porta fica fechada durante a troca inteira e só reabre depois da nova abertura.
        assert gateway.calls == ['open', 'ingress', 'snapshot', 'close', 'open', 'ingress']
        assert slot.phase == Phase.Rust and slot.lease is None
        assert slot.binding.key == key and slot.binding.generation == 2 and slot.binding.meta['terminal']['pane'] == '%7'
        await owner.change('session', lambda: asyncio.sleep(0), remove=True)
    asyncio.run(flow())
    import json
    state = json.loads(slot.binding.state_path.read_bytes())
    assert [row['text'] for row in state['rows']] == ['carry']


# --- Terminal nasce no Rust e o teclado emprestado (dono único, Task 6) ---

class LoanGateway(TerminalGateway):
    """Rust falso que empresta o teclado: guarda os pedidos de controle e o id do empréstimo."""
    def __init__(self, seconds_seen=None):
        super().__init__()
        self.controls = []
    async def op(self,target,command,operation_id,clock):
        if command['kind'] == 'control' and command['control'] in {'keyboard_loan','keyboard_return'}:
            self.calls.append(command['kind'])
            self.controls.append((command['control'], dict(command['payload'])))
            if command['control'] == 'keyboard_loan':
                return {'operation_id':operation_id,'disposition':'accepted','payload':{'loan_id':'loan:1:7','seconds':command['payload']['seconds']}}
            return {'operation_id':operation_id,'disposition':'accepted','payload':{'returned':True}}
        return await super().op(target,command,operation_id,clock)


def _born_terminal(monkeypatch, tmp_path, gateway):
    from app import runtime_terminal as terminal, pqueue
    from app.adapters.claude_headless import sessions
    from app.runtime_adapter import LegacyBridge
    collected = dict(name='session',pane='%1',created=1,namespace='socket:pid:birth',
        jsonl=str(tmp_path / 'sid.jsonl'),session_id='sid',config_dir=str(tmp_path),cwd=str(tmp_path),
        mux_argv=['tmux'],windows=False)
    monkeypatch.setattr(terminal, '_collect', lambda name: {**collected, 'name': name})
    monkeypatch.setattr(pqueue, '_queue_dir', lambda: tmp_path)
    monkeypatch.setattr(sessions, 'load', lambda name: None)
    owner = RuntimeCoordinator(gateway)
    owner.legacy = LegacyBridge(owner, {'claude': object()})
    rc._current = owner
    return owner


def test_terminal_session_registers_in_rust_without_python_phase(monkeypatch, tmp_path):
    gateway = TerminalGateway()
    owner = _born_terminal(monkeypatch, tmp_path, gateway)
    monkeypatch.setattr(owner, 'register', lambda binding: pytest.fail('registro Python do terminal'))
    async def flow():
        owner.loop = asyncio.get_running_loop()
        assert await owner.prepare_session('session', 'claude')
        slot = owner.slot('session')
        assert slot.phase == Phase.Rust and slot.lease is None and slot.store is None
        assert gateway.calls == ['open']
        await owner.change('session', lambda: asyncio.sleep(0), remove=True)
    asyncio.run(flow())


@pytest.mark.parametrize('guest', [True, False])
def test_guest_click_is_refused_when_the_click_itself_opens_the_terminal_in_rust(monkeypatch, tmp_path, guest):
    # Primeira operação depois de reiniciar o backend: ainda não há slot, e a fotografia da rota diria
    # "terminal fora do Rust". O próprio clique abre a sessão no Rust, e a recusa vem sob a barreira.
    from app import plugin_click, tmux, runtime_terminal as terminal
    gateway = LoanGateway()
    owner = _born_terminal(monkeypatch, tmp_path, gateway)
    sent = []
    def send_keys(name, keys, literal=False):
        terminal.assert_writer(name)
        sent.append(name)
        return True
    monkeypatch.setattr(tmux, 'send_keys', send_keys)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        assert 'session' not in owner.names and not owner.terminal_in_rust('session')
        marca = terminal.guest_admin.set(guest)
        try:
            if guest:
                with pytest.raises(terminal.GuestRefused):
                    await plugin_click._click('session', 2, 5)
            else:
                await plugin_click._click('session', 2, 5)
        finally:
            terminal.guest_admin.reset(marca)
        assert owner.slot('session').phase == Phase.Rust and gateway.calls.count('open') == 1
        if guest:
            assert sent == [] and gateway.controls == [], 'o convidado não recebe o teclado'
        else:
            assert sent == ['session'] and [kind for kind, _ in gateway.controls] == ['keyboard_loan', 'keyboard_return']
        await owner.change('session', lambda: asyncio.sleep(0), remove=True)
    asyncio.run(flow())


def test_admin_borrows_keyboard_without_moving_queue(monkeypatch, tmp_path):
    from app import runtime_terminal as terminal
    gateway = LoanGateway()
    owner = _born_terminal(monkeypatch, tmp_path, gateway)
    typed = []
    def action():
        terminal.assert_writer('session')       # é o que cada escrita no tmux confere
        typed.append(owner.slot('session').phase)
        return 'ok'
    async def flow():
        owner.loop = asyncio.get_running_loop()
        assert await owner.prepare_session('session', 'claude')
        assert await terminal.run_admin(owner, 'session', 'set_model', {'model':'haiku'}, action) == 'ok'
        slot = owner.slot('session')
        assert typed == [Phase.Rust], 'o Python digita com a sessão no Rust'
        assert slot.phase == Phase.Rust and slot.lease is None and slot.store is None, 'fila e trava ficam no Rust'
        assert [kind for kind, _ in gateway.controls] == ['keyboard_loan', 'keyboard_return']
        assert gateway.controls[1][1] == {'loan_id': 'loan:1:7'}
        assert 'close' not in gateway.calls and gateway.calls.count('open') == 1, 'sem fechar nem reabrir'
        with pytest.raises(RuntimeError):
            terminal.assert_writer('session')   # fora do empréstimo o Python não escreve
        await owner.change('session', lambda: asyncio.sleep(0), remove=True)
    asyncio.run(flow())


def test_keyboard_loan_expires_and_fails_with_code(monkeypatch, tmp_path):
    from app import runtime_terminal as terminal
    gateway = LoanGateway()
    owner = _born_terminal(monkeypatch, tmp_path, gateway)
    monkeypatch.setattr(terminal, '_LOAN_S', 1)        # prazo menos a folga: já vencido ao digitar
    def action():
        terminal.assert_writer('session')
    async def flow():
        owner.loop = asyncio.get_running_loop()
        assert await owner.prepare_session('session', 'claude')
        with pytest.raises(RuntimeError) as caught:
            await terminal.run_admin(owner, 'session', 'set_model', {'model':'haiku'}, action)
        assert getattr(caught.value, 'code', '') == 'keyboard_loan_expired'
        assert [kind for kind, _ in gateway.controls] == ['keyboard_loan', 'keyboard_return'], 'o teclado volta mesmo na falha'
        with pytest.raises(RuntimeError):
            terminal.assert_writer('session')
        await owner.change('session', lambda: asyncio.sleep(0), remove=True)
    asyncio.run(flow())


def test_lost_loan_reply_is_returned_by_repeating_the_same_id(monkeypatch, tmp_path):
    # O Rust concedeu e a resposta se perdeu: o mesmo pedido repetido traz a mesma concessão, que
    # volta na hora em vez de deixar o teclado preso até o prazo.
    from app import runtime_terminal as terminal

    class Lossy(LoanGateway):
        def __init__(self):
            super().__init__()
            self.loan_ids = []
        async def op(self, target, command, operation_id, clock):
            if command['kind'] == 'control' and command['control'] == 'keyboard_loan':
                self.loan_ids.append(operation_id)
                reply = await super().op(target, command, operation_id, clock)
                if len(self.loan_ids) == 1:
                    raise ConnectionResetError('resposta perdida')
                return reply
            return await super().op(target, command, operation_id, clock)

    gateway = Lossy()
    owner = _born_terminal(monkeypatch, tmp_path, gateway)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        assert await owner.prepare_session('session', 'claude')
        with pytest.raises(ConnectionResetError):
            await terminal.run_admin(owner, 'session', 'set_model', {'model':'haiku'}, lambda: pytest.fail('digitou sem teclado'))
        assert len(gateway.loan_ids) == 2 and gateway.loan_ids[0] == gateway.loan_ids[1], 'o mesmo id repetido'
        assert [kind for kind, _ in gateway.controls] == ['keyboard_loan', 'keyboard_loan', 'keyboard_return']
        await owner.change('session', lambda: asyncio.sleep(0), remove=True)
    asyncio.run(flow())


def test_closed_terminal_state_is_not_a_registration_error_at_boot(monkeypatch, tmp_path):
    # Sessão com terminal fechada deixa o estado da fila no disco: no boot ela não tem pane, e isso
    # não é falha de registro (a prova via 14 erros por restart, um por terminal fechado).
    from app import runtime_terminal as terminal, tmux, diag
    events = []
    monkeypatch.setattr(diag, 'registrar', lambda evento, nivel='ok', **campos: events.append((evento, campos.get('codigo'))))
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    owner.close_python_leases()
    rc._current = None
    monkeypatch.setattr(terminal, '_collect', lambda name: None)
    panes = {}
    monkeypatch.setattr(tmux, 'sessao_existe', lambda name: panes.get(name, False))
    fresh = RuntimeCoordinator()
    async def flow():
        await fresh._register_durable_terminals(owned=None)
    asyncio.run(flow())
    assert ('runtime.registration_failed', 'terminal_binding') not in events
    assert fresh.slot('session').awaiting_identity, 'o registro em espera continua'
    panes['session'] = True      # há sessão com o nome e o vínculo não se prova: aí é erro
    other = RuntimeCoordinator()
    asyncio.run(other._register_durable_terminals(owned=None))
    assert ('runtime.registration_failed', 'terminal_binding') in events
    events.clear()
    panes['session'] = None      # tmux sem resposta: não dá para dizer que fechou
    asyncio.run(RuntimeCoordinator()._register_durable_terminals(owned=None))
    assert ('runtime.registration_failed', 'terminal_binding') in events


def test_queue_read_waiting_for_rust_mode_does_not_block_the_handover(monkeypatch, tmp_path):
    """Boot com o Rust subindo: a leitura da fila (drain do SSE) espera o modo. Ela não pode segurar a
    fila Python enquanto espera, senão a passagem ao Rust recusa com 'fila da sessão em uso'."""
    from types import SimpleNamespace
    from app import runtime_queue
    gateway = TerminalGateway()
    owner, slot, _ = live_owner(monkeypatch, tmp_path, gateway=gateway)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        owner.mode = 'pending'
        reader = asyncio.ensure_future(asyncio.to_thread(runtime_queue.route_queue, SimpleNamespace(name='session'), 'load', {}))
        await asyncio.sleep(0.3)
        token = rc._mode_bypass.set(True)
        try:
            await owner._open_slot_in_rust('session', slot, launch=False)
        finally:
            rc._mode_bypass.reset(token)
        assert slot.phase == Phase.Rust
        owner._set_mode('rust')
        handled, _ = await asyncio.wait_for(reader, 5)
        assert handled and gateway.calls == ['open', 'queue']
    try:
        asyncio.run(flow())
    finally:
        if gateway.lease:
            gateway.lease.close()


def test_failure_reason_names_the_raise_site_without_copying_python_text(monkeypatch):
    from app import diag
    def runtime_raise():
        raise RuntimeError("texto da conversa")
    try:
        runtime_raise()
    except RuntimeError as exc:
        raised = exc
    reason = rc.failure_reason(raised)
    assert reason['codigo'] == 'RuntimeError' and reason['detalhe'] == ''
    assert reason['raise_site'].startswith('test_runtime_terminal.py:') and reason['raise_site'].endswith(' runtime_raise')
    records = []
    monkeypatch.setattr(diag, 'registrar', lambda event, level='ok', **fields: records.append(fields))
    rc._registration_failed('runtime.reopen_failed', 'session', raised)
    assert records[0]['raise_site'] == reason['raise_site']


def test_handover_waits_for_a_short_python_queue_write_and_refuses_a_stuck_one(monkeypatch, tmp_path):
    gateway = TerminalGateway()
    owner, slot, _ = live_owner(monkeypatch, tmp_path, gateway=gateway)
    monkeypatch.setattr(rc, '_ACTIVE_WAIT_S', 0.3)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        with slot.guard:
            slot.active += 1
        def release():
            with slot.guard:
                slot.active -= 1
            owner._signal(slot)
        owner.loop.call_later(0.05, release)
        await owner._open_slot_in_rust('session', slot, launch=False)
        assert slot.phase == Phase.Rust
    try:
        asyncio.run(flow())
    finally:
        if gateway.lease:
            gateway.lease.close()
    gateway = TerminalGateway()
    owner, slot, _ = live_owner(monkeypatch, tmp_path / 'stuck', gateway=gateway)
    async def stuck():
        owner.loop = asyncio.get_running_loop()
        with slot.guard:
            slot.active += 1
        with pytest.raises(RuntimeError, match='fila da sessão em uso'):
            await owner._open_slot_in_rust('session', slot, launch=False)
        assert slot.phase == Phase.Python and gateway.calls == []
        with slot.guard:
            slot.active -= 1
    asyncio.run(stuck())


def test_queue_inside_lifecycle_never_waits_for_the_mode(monkeypatch, tmp_path):
    from types import SimpleNamespace
    from app import runtime_queue
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    async def flow():
        owner.loop = asyncio.get_running_loop()
        owner.mode = 'pending'
        slot.lifecycle_token = object()
        def admin():
            token = rc._lifecycle.set(slot.lifecycle_token)
            try:
                return runtime_queue.route_queue(SimpleNamespace(name='session'), 'load', {})
            finally:
                rc._lifecycle.reset(token)
        handled, rows = await asyncio.wait_for(asyncio.to_thread(admin), 2)
        assert handled and rows == []
    asyncio.run(flow())


def test_rust_bypass_reopen_of_terminal_closes_and_reopens_in_rust(monkeypatch, tmp_path):
    from types import SimpleNamespace
    from app import api, runtime_terminal as terminal
    gateway = TerminalGateway()
    owner, slot, collected = live_owner(monkeypatch, tmp_path, gateway=gateway)
    facts = {'now': {**collected, 'session_proof': 'p1'}}
    proof = {'now': 'p1'}
    monkeypatch.setattr(terminal, '_collect', lambda name: {**facts['now'], 'name': name})
    monkeypatch.setattr(terminal, '_session_proof', lambda name: proof['now'])
    monkeypatch.setattr(terminal, '_pane_life', lambda name: None, raising=False)
    monkeypatch.setattr(api, '_headless', lambda name: False)
    monkeypatch.setattr(api, '_invalidate_lists', lambda: None)
    async def idle(name, headless):
        return None
    monkeypatch.setattr(api, '_motivo_ocupada', idle)
    calls = []
    def para_headless(name, mode):
        assert gateway.calls[-1] == 'close' and gateway.lease is None, 'o Rust soltou antes de matar o pane'
        calls.append(('para_headless', name, mode))
    def para_terminal(name):
        calls.append(('para_terminal', name))
        proof['now'] = 'p2'
        facts['now'] = {**collected, 'pane': '%7', 'created': 456, 'namespace': 'mux-novo', 'session_proof': 'p2'}
    monkeypatch.setattr(api.registry, 'para_headless', para_headless)
    monkeypatch.setattr(api.registry, 'para_terminal', para_terminal)
    async def flow():
        assert await owner.prepare_session('session', 'claude')
        assert slot.phase == Phase.Rust
        info = SimpleNamespace(name='session', provider='claude', headless=False, jsonl=None)
        result = await api._durante_troca('session', api._reabrir_em_bypass('session', info))
        assert result['reopened'] is True
        assert calls == [('para_headless', 'session', 'bypassPermissions'), ('para_terminal', 'session')]
        # Duas portas aninhadas: a da troca (`transfer_operation`) por fora e a do `change` por dentro.
        assert gateway.calls == ['open', 'ingress', 'ingress', 'snapshot', 'close', 'open', 'ingress', 'ingress']
        assert slot.phase == Phase.Rust and slot.lease is None and slot.binding.meta['terminal']['pane'] == '%7'
    asyncio.run(flow())


def test_clear_on_disk_counts_only_a_transcript_after_the_dispatch(tmp_path):
    import os
    from app import runtime_terminal as rt
    atual = tmp_path / 'atual.jsonl'
    atual.write_text('<command-name>/clear</command-name>\n')
    antigo = tmp_path / 'antigo.jsonl'
    antigo.write_text('{"message":{"content":"<command-name>/clear</command-name>"}}\n')
    os.utime(antigo, (1000, 1000))
    assert rt._clear_on_disk(str(atual), 2000) is False
    (tmp_path / 'novo.jsonl').write_text('{"message":{"content":"<command-name>/clear</command-name>"}}\n')
    assert rt._clear_on_disk(str(atual), 2000) is True


def test_terminal_facts_see_the_open_askuserquestion_sidecar(monkeypatch,tmp_path):
    # O sidecar do hook mora em <config>/.hangar-askq/<stem>.json: com o caminho inteiro do
    # transcript ele nunca era achado e os fatos diziam "sem pergunta" com o menu fora da tela.
    from app import runtime_terminal as terminal, askquestion
    owner,slot,collected=live_owner(monkeypatch,tmp_path)
    Path(collected['jsonl']).write_text('{"type":"assistant","timestamp":"2026-09-01T17:27:13.000Z","message":{"content":[{"type":"text","text":"oi"}]}}\n')
    (tmp_path/'.hangar-askq').mkdir()
    (tmp_path/'.hangar-askq'/'sid.json').write_text(json.dumps({'transcript_path':collected['jsonl'],
        'tool_input':{'questions':[{'question':'A ou B?','header':'Opção','multiSelect':False,
            'options':[{'label':'A','description':'a'},{'label':'B','description':'b'}]}]}}))
    monkeypatch.setattr(askquestion,'dirs_de_config',lambda:[tmp_path])
    descriptor=slot.binding.descriptor()
    metadata={**descriptor['meta'],'descriptor':descriptor,'operation_id':'root','validate':lambda:None}
    current=terminal.facts({'binding':descriptor['meta']['terminal'],'operation_id':'root','text':''},metadata)
    assert current['open_question'] is True


def test_clear_unknown_after_enter_raises_barrier_and_busy_session_keeps_it(monkeypatch,tmp_path):
    # Incerto depois do Enter (etapa *.submeter) pode ter trocado a conversa: trava. Com o Claude
    # trabalhando o /clear espera na fila do Claude Code, e a trava não sai no prazo.
    from app import runtime_terminal as rt, terminal_input as ti
    owner,slot,_ = live_owner(monkeypatch,tmp_path)
    def send(self,name,text):
        if text == '/clear':
            ti._ULTIMA_LIMPEZA.stage='linha.submeter'
            ti._ULTIMA_LIMPEZA.limpou=True
            return 'partial'
        return 'sent'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',send)
    async def flow():
        assert (await owner.op('session',{'kind':'submit','text':'/clear'},'clear'))['disposition'] == 'unknown'
        assert slot.store.state['runtime_state']['clear_barrier']['operation_id'] == 'clear'
        monkeypatch.setattr(rt,'CLEAR_APPLY_WAIT_S',0)
        monkeypatch.setattr(ti,'classify',lambda pane:('working','Pensando',None,None))
        with pytest.raises(RuntimeError): await owner.op('session',{'kind':'submit','text':'busy'},'busy')
        assert 'clear_barrier' in slot.store.state['runtime_state']
    asyncio.run(flow())


def test_legacy_clear_barrier_without_time_is_stamped_not_released(monkeypatch,tmp_path):
    from app import runtime_terminal as rt
    owner,slot,_ = live_owner(monkeypatch,tmp_path)
    state = copy.deepcopy(slot.store.state)
    state['runtime_state']['clear_barrier'] = {'generation':1,'conversation':'sid','operation_id':'old'}
    slot.store._persist(state)
    descriptor = slot.binding.descriptor()
    assert rt._expire_clear(owner, descriptor, 'probe') is False
    barrier = slot.store.state['runtime_state']['clear_barrier']
    assert barrier['raised'] > 0 and barrier['since'] == barrier['raised']


def test_clear_that_changes_conversation_answers_accepted_in_python(monkeypatch,tmp_path):
    # O /clear aplicado troca a conversa antes da limpeza da fila que o segue; a limpeza revalidava o
    # vínculo e o envio voltava 400 "vínculo terminal mudou" com o /clear feito.
    from app import terminal_input as ti
    owner,slot,collected = live_owner(monkeypatch,tmp_path)
    def send(self,name,text):
        collected['session_id']='after'
        collected['jsonl']=str(tmp_path / 'after.jsonl')
        return 'sent'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',send)
    async def flow():
        assert (await owner.op('session',{'kind':'submit','text':'/clear'},'clear'))['disposition'] == 'accepted'
    asyncio.run(flow())


def test_clear_unproved_before_enter_keeps_the_queue(monkeypatch,tmp_path):
    # O /clear que parou antes do Enter não rodou: as mensagens na fila não podem sumir com ele.
    from app import terminal_input as ti
    owner,slot,_ = live_owner(monkeypatch,tmp_path)
    slot.store.exec(1,'append',{'monotonic_s':1,'epoch_s':10},
        {'kind':'append','text':'na fila','delivered':False,'ts':10,'pre_transcript':False,'entry_id':'row'})
    def send(self,name,text):
        ti._ULTIMA_LIMPEZA.stage='linha.prova'
        ti._ULTIMA_LIMPEZA.limpou=False
        return 'partial'
    monkeypatch.setattr(ti.TerminalInput,'send_prompt',send)
    async def flow():
        assert (await owner.op('session',{'kind':'submit','text':'/clear'},'clear'))['disposition'] == 'unknown'
        assert [row['id'] for row in slot.store.state['rows']] == ['row']
    asyncio.run(flow())
