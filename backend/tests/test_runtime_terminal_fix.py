import asyncio

from app import api, terminal_input as ti
from test_runtime_terminal import isolated_owner, live_owner



async def _to_rust(owner, name):
    """Registro Python do terminal aberto no Rust (o caminho do vínculo pendente que provou a conversa)."""
    await owner._open_slot_in_rust(name, owner.slot(name), launch=False)
    return True

import json
import subprocess
from pathlib import Path

import pytest


@pytest.mark.parametrize('mode', ['fill', 'user', 'submit'])
@pytest.mark.parametrize('published_sid', ['A', 'B'])
def test_real_input_hook_refuses_publication_after_clear(tmp_path, mode, published_sid):
    root = Path(__file__).resolve().parents[2]
    runner = tmp_path / 'hook.mjs'
    runner.write_text('''import { readFileSync } from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';
const data = code => 'data:text/javascript;base64,' + Buffer.from(code).toString('base64');
const bridge = data(stripTypeScriptTypes(readFileSync(process.argv[2]+'/plugins/hangar/hooks/bridge.ts','utf8'), {mode:'strip'}));
const source = readFileSync(process.argv[2]+'/plugins/hangar/hooks/input.ts','utf8').replace('"./bridge"',JSON.stringify(bridge));
const {registerInput} = await import(data(stripTypeScriptTypes(source,{mode:'strip'})));
let start, sid='A'; const timers=[], effects=[], receipts=[];
registerInput((event,options,fn)=>{if(options.isInteractive) start=fn;});
const $={env:{get:async key=>({HANGAR_PLUGIN_URL:'http://fake',HANGAR_PLUGIN_TOKEN:'test',CP_SESSION_NAME:'session'})[key]},
 clock:{after:(ms,fn)=>timers.push(fn)}, session:{id:async()=>sid}, ui:{log:()=>{},invalidate:()=>{}},
 prompt:{fill:async()=>{effects.push({kind:'fill',sid});return {isFilled:true};},submit:async()=>effects.push({kind:'submit',sid})},
 http:{fetch:async(url,options)=>{if(url.endsWith('/pull')){sid='B';return {status:200,text:JSON.stringify({text:'text',modo:process.argv[3],publication_id:'pub-A',generation:1,session_id:process.argv[4]})};}
 receipts.push(JSON.parse(options.body)); return {status:200,text:'{}'};}}};
await start($,{},async event=>event); timers.shift()();
for(let i=0;i<100 && timers.length===0;i++) await new Promise(resolve=>setImmediate(resolve));
console.log(JSON.stringify({effects,receipts,rearmed:timers.length}));
''')
    # O prazo só flagra o hook preso: o primeiro node da etapa no runner Windows já passou de 10 s.
    result = subprocess.run(['node', str(runner), str(root), mode, published_sid], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    output = json.loads(result.stdout)
    assert output['effects'] == ([] if published_sid == 'A' else [{'kind':'fill' if mode == 'fill' else 'submit','sid':'B'}])
    assert output['rearmed'] == 1

from types import SimpleNamespace
from app import runtime_coordinator as rc, runtime_terminal as terminal, tmux, pqueue



def test_outside_provider_keeps_legacy(monkeypatch):
    owner = rc.RuntimeCoordinator()
    owner.legacy = SimpleNamespace(binding=lambda *args: None)
    monkeypatch.setattr(terminal, 'being_born', lambda name, after=0: False)
    monkeypatch.setattr(terminal, 'outside_scope', lambda name: True, raising=False)
    assert asyncio.run(owner.prepare_session('pi-session', 'claude')) is False


@pytest.mark.parametrize('state', ['unreadable_before', 'different_before', 'old_placeholder', 'unreadable_after', 'new_placeholder'])
def test_filled_reserve_needs_input_and_submission_proof(monkeypatch, tmp_path, state):
    owner, slot, _ = live_owner(monkeypatch, tmp_path)
    expected = 'mensagem para conferir'
    screen = {'value': 'old'}
    def pane(content): return '─'*30+'\n❯ '+content+'\n'+'─'*30+'\n'
    screen['value'] = pane('[Pasted text #1 +3 lines]')
    real_service = terminal._service
    def service(coordinator, descriptor, operation, request, kind, payload):
        if kind == 'terminal_facts':
            return dict(binding=payload['binding'], ready=True, idle=True, open_question=False,
                plugin_live=True, plugin_user=False, native=None, clipboard_available=False)
        if kind == 'terminal_publish':
            screen['value'] = ('???' if state == 'unreadable_before' else pane('different') if state == 'different_before'
                else pane('[Pasted text #1 +3 lines]') if state == 'old_placeholder' else pane('[Pasted text #2 +3 lines]') if state == 'new_placeholder' else pane(expected))
            return 'filled'
        return real_service(coordinator, descriptor, operation, request, kind, payload)
    monkeypatch.setattr(terminal, '_service', service)
    monkeypatch.setattr(ti, '_capture', lambda name: screen['value'])
    monkeypatch.setattr(ti, '_SUBMIT_CHECK_PRAZO', .03)
    monkeypatch.setattr(ti, '_SUBMIT_CHECK_INTERVALO', .001)
    effects = []
    def enter(*args, **kwargs):
        effects.append('Enter')
        screen['value'] = '???' if state == 'unreadable_after' else pane('')
        return True
    monkeypatch.setattr(tmux, 'send_keys', enter)
    async def scenario():
        result = await owner.op('session', {'kind':'submit','text':expected}, 'filled')
        assert result['disposition'] == ('accepted' if state == 'new_placeholder' else 'unknown')
        assert effects == (['Enter'] if state in {'unreadable_after','new_placeholder'} else [])
    asyncio.run(scenario())


import os
import shlex
import sys
import threading
import time
import uuid
from concurrent.futures import ThreadPoolExecutor


@pytest.mark.skipif(sys.platform == 'win32', reason='CLI falsa POSIX em mux próprio')
def test_python_multiline_buffers_do_not_cross_sessions(monkeypatch, tmp_path):
    label = 'hangar-fix-buffer-' + uuid.uuid4().hex
    cli = tmp_path / 'read.py'
    cli.write_text('''import os,sys,tty
fd=sys.stdin.fileno(); tty.setraw(fd); os.write(1,b'\\x1b[?2004hready'); data=b''; pending=b''; paste=False
while True:
 pending+=os.read(fd,4096)
 while pending:
  if pending.startswith(b'\\x1b'):
   if len(pending)<6:break
   if pending.startswith(b'\\x1b[200~'):paste=True;pending=pending[6:];continue
   if pending.startswith(b'\\x1b[201~'):paste=False;pending=pending[6:];continue
  char,pending=pending[:1],pending[1:]
  if char==b'\\r' and not paste:
   open(sys.argv[1]+'.tmp','wb').write(data);os.replace(sys.argv[1]+'.tmp',sys.argv[1]);sys.exit(0)
  data+=b'\\n' if char==b'\\r' and paste else char
''')
    raw = subprocess.run
    try:
        for name in ('a', 'b'):
            raw(['tmux','-L',label,'-f','/dev/null','new-session','-d','-s',name,
                shlex.join([sys.executable,str(cli),str(tmp_path / (name+'.out'))])], check=True)
        deadline = time.monotonic()+3
        for name in ('a','b'):
            while time.monotonic() < deadline:
                capture = raw(['tmux','-L',label,'capture-pane','-p','-t',f'={name}:'], capture_output=True,text=True)
                if 'ready' in capture.stdout: break
                time.sleep(.01)
            assert 'ready' in capture.stdout
        barrier = threading.Barrier(2)
        first_pasted = threading.Event()
        def run(args, **kwargs):
            if args[1] == 'paste-buffer' and '=b:' in args:
                assert first_pasted.wait(3)
            result = raw(args[:1]+['-L',label]+args[1:], **kwargs)
            if args[1] == 'load-buffer': barrier.wait(3)
            if args[1] == 'paste-buffer' and '=a:' in args: first_pasted.set()
            return result
        monkeypatch.setattr(tmux,'RUN',run)
        monkeypatch.setattr(tmux,'_pane_target',lambda name:f'={name}:')
        monkeypatch.setattr(tmux,'_TRUNCA_BUFFER',False)
        texts = {'a':'ação A 😀\nC:\\Users\\a','b':'ação B 😀\nC:\\Users\\b'}
        with ThreadPoolExecutor(max_workers=2) as pool:
            futures = [pool.submit(tmux.paste_text,name,text) for name,text in texts.items()]
            assert all(future.result(5) for future in futures)
        for name in ('a','b'):
            raw(['tmux','-L',label,'send-keys','-t',f'={name}:','-l','--','\r'],check=True)
        deadline = time.monotonic()+3
        while time.monotonic()<deadline and not all((tmp_path/(name+'.out')).exists() for name in texts):time.sleep(.01)
        assert {name:(tmp_path/(name+'.out')).read_bytes() for name in texts} == {name:text.encode() for name,text in texts.items()}
    finally:
        raw(['tmux','-L',label,'kill-server'],capture_output=True)


def test_terminal_being_born_waits_for_binding_instead_of_suspending(monkeypatch):
    # Pane recém-criado: o agente e a prova da conversa chegam segundos depois do pane.
    owner = rc.RuntimeCoordinator()
    bound = SimpleNamespace(key='terminal_k')
    answers = [None, None, bound]
    owner.legacy = SimpleNamespace(binding=lambda *args: answers.pop(0))
    monkeypatch.setattr(terminal, 'being_born', lambda name, after=0: True)
    monkeypatch.setattr(rc, '_BIRTH_POLL_S', 0)
    registered = []
    monkeypatch.setattr(owner, 'register', lambda binding: registered.append(binding))
    assert asyncio.run(owner.prepare_session('session', 'claude')) is False
    assert registered == [bound] and answers == []


def test_terminal_not_being_born_still_suspends(monkeypatch):
    owner = rc.RuntimeCoordinator()
    calls = []
    owner.legacy = SimpleNamespace(binding=lambda *args: calls.append(1))
    monkeypatch.setattr(terminal, 'being_born', lambda name, after=0: False)
    monkeypatch.setattr(terminal, 'outside_scope', lambda name: False)
    with pytest.raises(RuntimeError, match='escrita suspensa'):
        asyncio.run(owner.prepare_session('session', 'claude'))
    assert len(calls) == 1


@pytest.mark.parametrize('created_ago,after,outside,expected', [
    (2, 0, False, True), (600, 0, False, False), (2, 0, True, False), (2, 'later', False, False)])
def test_being_born_needs_young_pane_of_a_new_life(monkeypatch, created_ago, after, outside, expected):
    import time
    now = int(time.time())
    monkeypatch.setattr(tmux, 'list_panes_all', lambda: {'session': [{'session_created': now - created_ago}]})
    monkeypatch.setattr(terminal, 'outside_scope', lambda name: outside)
    assert terminal.being_born('session', now + 1 if after == 'later' else after) is expected
    assert terminal.being_born('missing') is False
