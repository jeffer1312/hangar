"""Referências adicionais executadas nas fontes Python originais, fora de estado real."""
import argparse
import asyncio
import hashlib
import json
import logging
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import types
SOURCE='ba2db1021ab7fb801eca50a50f9509dcb825ac7e'
REPO=Path(__file__).resolve().parents[5]

async def capture(root):
    import app
    modules={}
    for name in ('navsock','guest_user_gate'):
        m=types.ModuleType('original_'+name)
        source=root/(name+'.py')
        exec(compile(source.read_text(),str(source),'exec'),m.__dict__)
        modules[name]=m
    nav,gate=modules['navsock'],modules['guest_user_gate']
    result={'guest_tokens':[], 'opening':[], 'debug':[]}
    cases=[
      ('query_first_nonempty',[], 'token=&token=login-guest&token=owner',False),
      ('query_owner_before_guest',[], 'token=owner&token=login-guest',False),
      ('last_authorization',[['authorization','Bearer owner'],['authorization','Bearer login-guest']], 'token=owner',False),
      ('bearer_empty',[['authorization','Bearer ']], 'token=login-guest',False),
      ('host_cookie',[['cookie','cp_token=owner; __Host-cp_token=login-guest']], '',True),
      ('https_plain_cookie',[['cookie','cp_token=login-guest']], '',True),
      ('last_cookie',[['cookie','cp_token=owner'],['cookie','cp_token=login-guest']], '',False),
      ('quoted_cookie',[['cookie',r'cp_token="login\055guest"']], '',False),
      ('invalid_cookie',[['cookie','broken; cp_token=login-guest']], '',False),
      ('empty_host_cookie',[['cookie','__Host-cp_token=; cp_token=login-guest']], '',False),
    ]
    for name,headers,query,https in cases:
        scope={'type':'websocket','scheme':'wss' if https else 'ws','headers':[(k.encode(),v.encode()) for k,v in headers], 'query_string':query.encode()}
        result['guest_tokens'].append({'name':name,'headers':headers,'query':query,'https':https,'token':gate._token(scope)})
    for name in ('http_401','http_403','connection_refused','handshake_timeout'):
        held=[]
        if name=='connection_refused':
            reserved=socket.socket();reserved.bind(('127.0.0.1',0));port=reserved.getsockname()[1]
            server=None
        else:
            async def accept(reader,writer):
                held.append(writer)
                try:
                    await reader.readuntil(b'\r\n\r\n')
                    if name=='handshake_timeout':
                        await reader.read()
                    else:
                        status,reason=(401,'Unauthorized') if name=='http_401' else (403,'Forbidden')
                        writer.write(f'HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n'.encode());await writer.drain()
                finally:
                    writer.close();await writer.wait_closed()
            server=await asyncio.start_server(accept,'127.0.0.1',0);port=server.sockets[0].getsockname()[1]
        try:
            try:
                async with nav._Cdp(f'ws://127.0.0.1:{port}/cdp',{'Authorization':'Bearer isolated-desktop-secret'}):
                    raise AssertionError('abertura deveria falhar')
            except Exception as error:
                result['opening'].append({'name':name,'type':type(error).__name__, 'message':str(error).replace(str(port),'<PORT>'), 'repr':repr(error).replace(str(port),'<PORT>')})
        finally:
            if server:
                server.close();await server.wait_closed()
            else: reserved.close()
            for writer in held:
                writer.close();await writer.wait_closed()
    class Collect(logging.Handler):
        def emit(self,record):
            result['debug'].append({'level':record.levelname,'message':record.getMessage()})
    collector=Collect();old_level=nav._log.level;nav._log.setLevel(logging.DEBUG);nav._log.addHandler(collector)
    class Cdp:
        async def cmd(self,*args,**kwargs):raise TimeoutError()
    class Screen:
        async def send_text(self,*args):raise AssertionError('falha de reserva não ganha evento')
    try:
        await nav._print_avulso(Cdp(),Screen(),{'w':800,'h':600,'qualidade':45})
        url=await nav._url_atual(Cdp(),{'targetId':'active','url':'http://contract.test/fallback'})
        result['fallback_url']=url
    finally:
        nav._log.removeHandler(collector);nav._log.setLevel(old_level)
    return result

def main():
    p=argparse.ArgumentParser();p.add_argument('--child',type=Path);p.add_argument('--output',type=Path,default=Path('/tmp/hangar-browser-final-review.json'));a=p.parse_args()
    if a.child:
        (a.child/'result.json').write_text(json.dumps(asyncio.run(capture(a.child)),ensure_ascii=False,indent=2)+'\n');return
    raw={name:subprocess.check_output(['git','show',f'{SOURCE}:backend/app/{name}.py'],cwd=REPO) for name in ('navsock','guest_user_gate')}
    with tempfile.TemporaryDirectory(prefix='hangar-browser-final-original-') as temp:
        root=Path(temp);home=root/'home';home.mkdir()
        for name,data in raw.items():(root/(name+'.py')).write_bytes(data)
        env={'HOME':str(home),'USERPROFILE':str(home),'XDG_CONFIG_HOME':str(home/'.config'),'XDG_CACHE_HOME':str(home/'.cache'),'XDG_DATA_HOME':str(home/'.local/share'),'CLAUDE_CONFIG_DIR':str(home/'.claude'),'CODEX_HOME':str(home/'.codex'),'PATH':'/usr/bin:/bin','PYTHONPATH':str(REPO/'backend'),'PYTHONDONTWRITEBYTECODE':'1','PYTHONNOUSERSITE':'1','NO_PROXY':'127.0.0.1,localhost','CP_AUTH_TOKEN':'isolated-owner','CP_SCAN_ROOTS':str(root/'workspace'),'CP_PROJECTS_DIR':str(root/'workspace')}
        completed=subprocess.run([sys.executable,str(Path(__file__).resolve()),'--child',str(root)],env=env,cwd=root,capture_output=True,text=True,timeout=20)
        if completed.returncode:raise RuntimeError(completed.stderr)
        observed=json.loads((root/'result.json').read_text())
    output={'source_commit':SOURCE,'source_sha256':{f'backend/app/{name}.py':hashlib.sha256(data).hexdigest() for name,data in raw.items()},'generator':'/home/jefferson/Projetos/hangar/backend/.venv/bin/python /tmp/generate_hangar_browser_final_review.py','python_version':sys.version.split()[0],'websockets_version':__import__('websockets').__version__,'platform':sys.platform,'comparison':{'normalized_fields':['opening.connection_refused.message/repr: própria porta efêmera vira <PORT>']},'limits':['Captura de _Cdp.__aenter__, _token e dois registros DEBUG originais, não desktop/cliente reais.','HTTP401/403 mínimos em porta de ensaio; conexão recusada usa porta própria reservada sem listen.','Prazo real5s de handshake, não simulado; comando CDP TimeoutError() continua caso separado.'],'observed':observed}
    a.output.write_text(json.dumps(output,ensure_ascii=False,indent=2)+'\n');print(json.dumps({'output':str(a.output),'opening':observed['opening'],'debug':observed['debug']},ensure_ascii=False))
if __name__=='__main__':main()
