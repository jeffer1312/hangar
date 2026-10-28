"""Corpus da autenticação Python original; não implementa contador nem abre sessões."""
from __future__ import annotations
import argparse
import ast
import asyncio
import base64
import binascii
from collections import Counter
import contextvars
import hashlib
import hmac
import json
import logging
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import threading
import types
from typing import Optional
from fastapi import HTTPException, Request, WebSocket

SOURCE='ba2db1021ab7fb801eca50a50f9509dcb825ac7e'
REPO=Path(__file__).resolve().parents[5]
OWNER='auth-reference-owner-fake'
WRONG='auth-reference-wrong-fake'
IP='198.51.100.71'
CONNECT='192.0.2.1'
FILES=('auth','navsock','voicesock','termsock','guest_header','mensagens','connect_port')
SYMBOLS={
 'auth':['_MAX_FAILS','_WINDOW','_MAX_ORIGINS','_fails','_diag_recusas','_diag_lock','_LOOPBACK','COOKIE','COOKIE_HOST','_COOKIE_METODOS','_log','registrar_acesso','cookie_token','_blocked','_evict','_record_fail','reset_backoff','require_auth'],
 'navsock':['_autorizado'],
 'voicesock':['is_guest','_owner_only'],
 'termsock':['_porta_de_entrada'],
 'guest_header':['HEADER','_SCOPE_KEY','GuestView','guest_of','_verify'],
 'mensagens':['erro'],
 'connect_port':['CONNECT_PEER'],
}

def extract(source, names, namespace):
    parsed=ast.parse(source.read_text(),filename=str(source))
    selected=[];seen=[]
    for node in parsed.body:
        defined=[]
        if isinstance(node,(ast.FunctionDef,ast.AsyncFunctionDef,ast.ClassDef)):defined=[node.name]
        elif isinstance(node,ast.Assign):defined=[t.id for t in node.targets if isinstance(t,ast.Name)]
        elif isinstance(node,ast.AnnAssign) and isinstance(node.target,ast.Name):defined=[node.target.id]
        if any(name in names for name in defined):selected.append(node);seen+=defined
    if set(names)-set(seen):raise AssertionError(f'símbolos ausentes em {source.name}: {set(names)-set(seen)}')
    exec(compile(ast.Module(body=selected,type_ignores=[]),str(source),'exec'),namespace)

class Clock:
    def __init__(self):self.now=1000.0
    def time(self):return self.now

async def capture(root):
    clock=Clock();diagnostics=[];logs=[];external=[]
    old_modules={name:sys.modules.get(name) for name in ('app','app.auth','app.guest_header','app.guest_users')}
    app=types.ModuleType('app');app.__path__=[]
    current=contextvars.ContextVar('isolated_guest_context',default=None)
    guests=types.ModuleType('app.guest_users');guests.current=current
    app.guest_users=guests;sys.modules['app']=app;sys.modules['app.guest_users']=guests
    class Diag:
        @staticmethod
        def registrar(event,status,**fields):diagnostics.append({'event':event,'status':status,'fields':fields})
    class Mux:
        class MuxIndisponivel(Exception):pass
        @staticmethod
        def has_session(name):external.append({'call':'tmux.has_session','name':name});return True
    def origin_boundary(*args):raise AssertionError('o corpus de contador não deve decidir Origin de convidado')
    common={'asyncio':asyncio,'logging':logging,'secrets':secrets,'threading':threading,'time':clock,
            'settings':types.SimpleNamespace(auth_token=OWNER,public_url=''),'Request':Request,'WebSocket':WebSocket,
            'HTTPException':HTTPException,'Optional':Optional,'diag':Diag,'guest_users':guests,'tmux':Mux,
            '_origem_aceita':origin_boundary,'base64':base64,'binascii':binascii,'hashlib':hashlib,'hmac':hmac,'json':json}
    mods={}
    try:
        for name in ('connect_port','mensagens'):
            ns={'__name__':'original.'+name,**common};extract(root/(name+'.py'),SYMBOLS[name],ns);mods[name]=types.SimpleNamespace(**ns)
        common['CONNECT_PEER']=mods['connect_port'].CONNECT_PEER
        common['erro']=mods['mensagens'].erro
        header=types.ModuleType('app.guest_header');header.__dict__.update(common)
        header.internal_api=types.SimpleNamespace(_secret=None,_LOOPBACK={'127.0.0.1','::1','localhost'})
        extract(root/'guest_header.py',SYMBOLS['guest_header'],header.__dict__)
        app.guest_header=header;sys.modules['app.guest_header']=header
        common['guest_of']=header.guest_of
        auth=types.ModuleType('app.auth');auth.__dict__.update(common);extract(root/'auth.py',SYMBOLS['auth'],auth.__dict__)
        app.auth=auth;sys.modules['app.auth']=auth;mods['auth']=auth
        for name in ('navsock','voicesock','termsock'):
            ns={'__name__':'original.'+name,**common,'_LOOPBACK':auth._LOOPBACK,'_blocked':auth._blocked,'_record_fail':auth._record_fail}
            extract(root/(name+'.py'),SYMBOLS[name],ns);mods[name]=types.SimpleNamespace(**ns)
        class Collect(logging.Handler):
            def emit(self,record):logs.append({'level':record.levelname,'message':record.getMessage()})
        collector=Collect();previous_level=auth._log.level;auth._log.addHandler(collector);auth._log.setLevel(logging.DEBUG)
        def snapshot():
            return {'origin_order':list(auth._fails),'origins':[{'ip':ip,'hits':list(hits)} for ip,hits in auth._fails.items()],
                    'access_diagnostics':[{'domain':key[0],'ip':key[1],'mechanism':value[0],'reason':value[1]} for key,value in auth._diag_recusas.items()]}
        async def execute(step):
            clock.now=float(step.get('now',1000));profile=step['profile'];ip=step.get('ip',IP)
            if profile=='record':
                result=auth._record_fail(ip,clock.time());return {'return':result}
            if profile=='blocked':return {'return':auth._blocked(ip,clock.time())}
            query=[] if step.get('token') is None else [('token',step['token'])]
            from urllib.parse import urlencode
            scope={'type':'http' if profile=='http' else 'websocket','method':'GET','scheme':'http' if profile=='http' else 'ws',
                   'path':step.get('path','/api/me'),'raw_path':step.get('path','/api/me').encode(),
                   'query_string':urlencode(query).encode(),'headers':[(b'host',b'isolated.invalid')],
                   'client':(ip,12345),'server':('127.0.0.1',8765),'http_version':'1.1'}
            if step.get('guest')=='signed':scope['hangar.guest']=header.GuestView({'isolated-session':'share'})
            marker=current.set(types.SimpleNamespace(id='fake-login-guest')) if step.get('guest')=='login' else None
            try:
                if profile=='http':
                    request=Request(scope)
                    try:return {'return':auth.require_auth(request)}
                    except HTTPException as error:return {'raised':{'type':type(error).__name__,'status_code':error.status_code,'detail':error.detail,'headers':error.headers}}
                messages=[]
                async def receive():raise AssertionError('uma guarda não deve abrir/ler terminal ou navegador')
                async def send(message):messages.append(message)
                ws=WebSocket(scope,receive,send)
                if profile=='browser_ws':returned=await mods['navsock']._autorizado(ws)
                elif profile=='voice_ws':returned=await mods['voicesock']._owner_only(ws)
                elif profile=='terminal_ws':returned=await mods['termsock']._porta_de_entrada(ws,'isolated-session')
                else:raise AssertionError(profile)
                return {'return':returned,'messages':messages}
            finally:
                if marker is not None:current.reset(marker)
        def request(profile,token=WRONG,ip=IP,now=1000,path=None,guest=None):
            result={'profile':profile,'ip':ip,'token':token,'now':now}
            if path is not None:result['path']=path
            if guest is not None:result['guest']=guest
            return result
        def record(ip=IP,now=1000):return {'profile':'record','ip':ip,'now':now}
        def blocked(now,ip=IP):return {'profile':'blocked','ip':ip,'now':now}
        cases=[]
        async def case(name,setup,steps):
            auth.reset_backoff();diagnostics.clear();logs.clear();external.clear();clock.now=1000.0
            for step in setup:await execute(step)
            state=snapshot();observed=[]
            for step in steps:
                result=await execute(step);observed.append({'input':step,'output':result,'state':snapshot()})
            cases.append({'name':name,'setup':setup,'setup_state':state,'steps':observed,'final_state':snapshot(),
                          'diagnostic_calls':list(diagnostics),'warnings':list(logs),'external_calls':list(external)})
        setup7=[record() for _ in range(7)]
        await case('mixed_http_browser_me_then_eighth_ws_blocks_correct',[],[
            *[request('http',path='/api/sessions/isolated-session/navegador' if n%2==0 else '/api/me') for n in range(7)],
            request('browser_ws'),request('http',OWNER,path='/api/me'),request('browser_ws',OWNER)])
        await case('http_success_clears_immediately',setup7,[request('http',OWNER),request('browser_ws')])
        await case('browser_ws_success_does_not_clear',setup7,[request('browser_ws',OWNER),request('browser_ws'),request('http',OWNER)])
        await case('connect_http_missing_does_not_count_and_success_does_not_clear',[record(CONNECT) for _ in range(7)],
                   [request('http',None,CONNECT),request('http',OWNER,CONNECT),request('http',WRONG,CONNECT),request('http',OWNER,CONNECT)])
        await case('connect_browser_ws_missing_counts',[],[request('browser_ws',None,CONNECT) for _ in range(8)]+[request('browser_ws',OWNER,CONNECT)])
        await case('connect_voice_ws_missing_does_not_count',[],[request('voice_ws',None,CONNECT) for _ in range(8)]+
                   [request('voice_ws',WRONG,CONNECT) for _ in range(7)]+[request('voice_ws',OWNER,CONNECT),request('voice_ws',WRONG,CONNECT),request('voice_ws',OWNER,CONNECT)])
        await case('connect_terminal_ws_missing_counts',[],[request('terminal_ws',None,CONNECT) for _ in range(8)]+[request('terminal_ws',OWNER,CONNECT)])
        for label,ip in [('ipv4','127.0.0.1'),('ipv6','::1'),('literal','localhost')]:
            await case('loopback_'+label,[],[request(profile,WRONG,ip) for profile in ('http','browser_ws','voice_ws','terminal_ws') for _ in range(8)]+
                       [request(profile,OWNER,ip) for profile in ('http','browser_ws','voice_ws','terminal_ws')])
        await case('window_full_29_999_and_exact_30',[record() for _ in range(8)],[blocked(1029.999),blocked(1030),request('http',OWNER,now=1030)])
        await case('window_partial_expiry',[record(now=1000) for _ in range(4)]+[record(now=1005) for _ in range(4)],
                   [blocked(1029.999),blocked(1030),blocked(1034.999),blocked(1035)])
        await case('clock_rollback_keeps_future_hits_blocked',[record() for _ in range(8)],[blocked(999),request('http',OWNER,now=999),blocked(1030)])
        await case('clock_rollback_appends_in_call_order',setup7,[request('http',WRONG,now=999),blocked(1029),blocked(1029.999),blocked(1030)])
        origins=[f'198.18.{n//256}.{n%256}' for n in range(514)]
        await case('capacity_513_origins_same_timestamp_insertion_tie',[record(ip) for ip in origins[:513]],[])
        await case('clear_remove_reinsert_enters_last_before_tie_eviction',[record(ip) for ip in origins[:513]],
                   [request('http',OWNER,origins[1]),record(origins[1]),record(origins[513])])
        await case('capacity_evicts_expired_before_same_time_live_origins',[record(origins[0],970)]+[record(ip) for ip in origins[1:513]],[])
        await case('guests_use_original_helper_before_or_after_block_as_declared',setup7,
                   [request('http',None,guest='signed'),request('http',None,guest='login'),
                    request('browser_ws',None,guest='signed'),request('terminal_ws',None,guest='signed'),
                    request('terminal_ws',None,guest='login'),request('voice_ws',None,guest='signed'),request('voice_ws',None,guest='login'),
                    record(),request('http',None,guest='signed'),request('http',None,guest='login'),
                    request('browser_ws',None,guest='signed'),request('terminal_ws',None,guest='signed'),request('voice_ws',None,guest='signed')])
        pending=[task.get_name() for task in asyncio.all_tasks() if task is not asyncio.current_task() and not task.done()]
        if pending:raise AssertionError(f'tarefas sobreviventes: {pending}')
        return {'cases':cases,'remaining_async_tasks':pending}
    finally:
        if 'auth' in mods:
            auth._log.removeHandler(collector);auth._log.setLevel(previous_level)
            auth.reset_backoff()
        for name,module in old_modules.items():
            if module is None:sys.modules.pop(name,None)
            else:sys.modules[name]=module

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--child',type=Path);parser.add_argument('--output',type=Path,default=Path('/tmp/hangar-auth-reference.json'));args=parser.parse_args()
    if args.child:
        observed=asyncio.run(capture(args.child))
        alive=[thread.name for thread in threading.enumerate() if thread is not threading.main_thread()]
        if alive:raise AssertionError(f'threads sobreviventes após asyncio.run: {alive}')
        observed['remaining_threads']=alive
        (args.child/'result.json').write_text(json.dumps(observed,ensure_ascii=False,indent=2)+'\n');return
    raw={name:subprocess.check_output(['git','show',f'{SOURCE}:backend/app/{name}.py'],cwd=REPO) for name in FILES}
    with tempfile.TemporaryDirectory(prefix='hangar-auth-original-') as temp:
        root=Path(temp);home=root/'home';home.mkdir()
        for name,data in raw.items():(root/(name+'.py')).write_bytes(data)
        env={'PATH':'/usr/bin:/bin','HOME':str(home),'USERPROFILE':str(home),'XDG_CONFIG_HOME':str(home/'.config'),
             'XDG_DATA_HOME':str(home/'.local/share'),'XDG_CACHE_HOME':str(home/'.cache'),'CLAUDE_CONFIG_DIR':str(home/'.claude'),
             'CODEX_HOME':str(home/'.codex'),'PYTHONNOUSERSITE':'1','PYTHONDONTWRITEBYTECODE':'1'}
        completed=subprocess.run([sys.executable,str(Path(__file__).resolve()),'--child',str(root)],cwd=root,env=env,capture_output=True,text=True,timeout=15)
        if completed.returncode:raise RuntimeError(completed.stderr)
        observed=json.loads((root/'result.json').read_text())
    counts=Counter(step['profile'] for case in observed['cases'] for step in case['setup'])
    counts.update(step['input']['profile'] for case in observed['cases'] for step in case['steps'])
    output={'source_commit':SOURCE,'sources':{f'backend/app/{name}.py':{'sha256':hashlib.sha256(data).hexdigest(),'text':data.decode('utf-8'),'extracted_symbols':SYMBOLS[name]} for name,data in raw.items()},
            'generator':'/home/jefferson/Projetos/hangar/backend/.venv/bin/python /tmp/generate_hangar_auth_reference.py',
            'python_version':sys.version.split()[0],'owner_token':OWNER,'clock':{'start':1000,'kind':'atributo time de cada módulo isolado aponta ao mesmo Clock; time global não é alterado'},
            'isolation':'subprocesso env_clear/HOME/config/cwd temporários; AST das funções e constantes exatas; Request/WebSocket Starlette reais com bordas ASGI sem rede',
            'boundaries':['guest_header.guest_of/GuestView/_verify originais; scope de convidado já verificado ou internal_api sem segredo para não convidado.',
                          'guest_users.current é ContextVar externo; nenhum cadastro/login ou arquivo real é consultado.',
                          'diag.registrar coleta chamadas em memória; tmux.has_session é uma fonte externa que só retornaTrue e anota a chamada.',
                          'Os imports são supridos no namespace, sem importar módulos Hangar inteiros ou executar helpers de PTY/CDP/voz.'],
            'limits':['Helpers no mesmo processo Python compartilham _fails; não substitui a prova Rust+Python em duas portas/processos.',
                      'HTTP de sucesso é retornoNone de require_auth; não afirma corpo/status de um endpoint não executado.',
                      'WS registra o retorno da guarda e as mensagens ASGI reais antes do aceite; não executa handshake de rede nem abre PTY/voz/navegador.',
                      'Casos são sequenciais; não altera secrets nem inventa interleaving de compare_digest para criar corrida Python.',
                      'Atomicidade RustA07 é contrato aprovado adicional, não esperada a partir de corrida não determinística Python.',
                      'Capacidade:513origens com o mesmo tempo, estado e ordem completos preservados; não ordena dict antes de comparar.'],
            'methodology':{'case_count':len(observed['cases']),'helper_invocations_including_setup':sum(counts.values()),'by_profile':dict(counts)},**observed}
    args.output.write_text(json.dumps(output,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps({'output':str(args.output),**output['methodology'],'remaining_async_tasks':output['remaining_async_tasks'],'remaining_threads':output['remaining_threads']},ensure_ascii=False))
if __name__=='__main__':main()
