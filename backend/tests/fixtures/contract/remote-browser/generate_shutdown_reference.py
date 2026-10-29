"""Parada normal do servidor Python original com controlador vivo e espectador aberto."""
import argparse
import asyncio
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import types
from fastapi import FastAPI, Request, WebSocket
from fastapi.responses import PlainTextResponse
SOURCE='ba2db1021ab7fb801eca50a50f9509dcb825ac7e'
REPO=Path(__file__).resolve().parents[5]
OWNER='isolated-shutdown-owner'
async def start(app):
    import uvicorn
    ready=asyncio.Event()
    class Server(uvicorn.Server):
        async def startup(self,sockets=None):
            await super().startup(sockets=sockets);ready.set()
    socket_=socket.socket();socket_.bind(('127.0.0.1',0));port=socket_.getsockname()[1]
    server=Server(uvicorn.Config(app,lifespan='off',log_level='critical'))
    task=asyncio.create_task(server.serve(sockets=[socket_]))
    await asyncio.wait_for(ready.wait(),4)
    return server,task,port
async def capture(root):
    import app
    import websockets
    from starlette.websockets import WebSocketDisconnect
    for name in ('navsock','navshell'):
        module=types.ModuleType('app.'+name);source=root/(name+'.py')
        exec(compile(source.read_text(),str(source),'exec'),module.__dict__)
        sys.modules['app.'+name]=module;setattr(app,name,module)
    original=app.navsock
    nav=Path.home()/'.hangar/nav';nav.mkdir(parents=True)
    measured=json.loads((REPO/'backend/tests/fixtures/contract/remote-browser/chromium.json').read_text())
    key='contract::nav-contract';name='nav-contract';requests=[];events=[];layout='mobile'
    desktop=FastAPI();public=FastAPI()
    @desktop.post('/cmd')
    async def command(request:Request):
        nonlocal layout
        body=await request.json();requests.append({'authorization':request.headers.get('authorization'),'body':body,'receiver_pid':123})
        if body['args']:layout='x'.join(body['args'])
        return PlainTextResponse('layout: '+layout)
    @desktop.websocket('/cdp')
    async def cdp(ws:WebSocket):
        await ws.accept()
        try:
            while True:
                command=json.loads(await ws.receive_text())
                await ws.send_text(json.dumps({'id':command['id'],'result':{}}))
                if command['method']=='Page.startScreencast':
                    await ws.send_text(json.dumps({'method':'Page.screencastFrame','params':measured['frame']}))
        except WebSocketDisconnect:pass
    @public.websocket('/api/sessions/nav-contract/nav-remoto')
    async def remote(ws:WebSocket):await original.nav_ws(ws,name,lambda _:True)
    desktop_server,desktop_task,port=await start(desktop)
    (nav/'browser.json').write_text(json.dumps({'chave':key,'pid':123,'targetId':None,'url':'http://contract.test/original'}))
    (nav/'_srv.json').write_text(json.dumps({'porta':port,'pid':123,'token':'remote-browser-contract-desktop'}))
    server,task,public_port=await start(public)
    try:
        async with websockets.connect(f'ws://127.0.0.1:{public_port}/api/sessions/{name}/nav-remoto?token={OWNER}') as client:
            while True:
                event=json.loads(await asyncio.wait_for(client.recv(),4));events.append(event)
                if event['t']=='q':break
            await client.send(json.dumps({'t':'layout','w':900,'h':700}))
            while True:
                event=json.loads(await asyncio.wait_for(client.recv(),4));events.append(event)
                if event['t']=='layout':break
            assert layout=='900x700'
            server.should_exit=True
            await asyncio.wait_for(task,5)
            await asyncio.wait_for(client.wait_closed(),2)
            result={'events_before_stop':events,'cmd':requests,'desktop_layout':layout,'client_close':{'code':client.close_code,'reason':client.close_reason},'record_after_stop':original._imposto.get(name),'viewer_closed_by_test_before_stop':False}
    finally:
        server.should_exit=True;desktop_server.should_exit=True
        await asyncio.wait_for(asyncio.gather(task,desktop_task),5)
    return result

def main():
    p=argparse.ArgumentParser();p.add_argument('--child',type=Path);p.add_argument('--output',type=Path,default=Path('/tmp/hangar-browser-shutdown-reference.json'));a=p.parse_args()
    if a.child:
        (a.child/'result.json').write_text(json.dumps(asyncio.run(capture(a.child)),ensure_ascii=False,indent=2)+'\n');return
    raw={name:subprocess.check_output(['git','show',f'{SOURCE}:backend/app/{name}.py'],cwd=REPO) for name in ('navsock','navshell')}
    with tempfile.TemporaryDirectory(prefix='hangar-browser-stop-original-') as temp:
        root=Path(temp);home=root/'home';home.mkdir()
        for name,data in raw.items():(root/(name+'.py')).write_bytes(data)
        env={'HOME':str(home),'USERPROFILE':str(home),'XDG_CONFIG_HOME':str(home/'.config'),'XDG_CACHE_HOME':str(home/'.cache'),'XDG_DATA_HOME':str(home/'.local/share'),'CLAUDE_CONFIG_DIR':str(home/'.claude'),'CODEX_HOME':str(home/'.codex'),'PATH':'/usr/bin:/bin','PYTHONPATH':str(REPO/'backend'),'PYTHONDONTWRITEBYTECODE':'1','PYTHONNOUSERSITE':'1','NO_PROXY':'127.0.0.1,localhost','CP_AUTH_TOKEN':OWNER,'CP_SCAN_ROOTS':str(root/'workspace'),'CP_PROJECTS_DIR':str(root/'workspace')}
        completed=subprocess.run([sys.executable,str(Path(__file__).resolve()),'--child',str(root)],env=env,cwd=root,capture_output=True,text=True,timeout=15)
        if completed.returncode:raise RuntimeError(completed.stderr)
        observed=json.loads((root/'result.json').read_text())
    output={'source_commit':SOURCE,'source_sha256':{f'backend/app/{name}.py':hashlib.sha256(data).hexdigest() for name,data in raw.items()},'generator':'/home/jefferson/Projetos/hangar/backend/.venv/bin/python /tmp/generate_hangar_browser_shutdown_reference.py','python_version':sys.version.split()[0],'websockets_version':__import__('websockets').__version__,'isolation':'filho env_clear/HOME/config/cwd temporários; dois servidoresHTTP/WS de loopback próprios; desktop permanece aberto durante parada do servidororiginal','limits':['Um caso Linux, sem sessão ou desktop reais.','Parada normal do Uvicorn original; não representa queda abrupta do processo ou orquestração inteira do Supervisor.','O prazo5s é limite de conclusão do ensaio responsivo, não reduz timeout45s do controlador.'],'observed':observed}
    a.output.write_text(json.dumps(output,ensure_ascii=False,indent=2)+'\n');print(json.dumps({'output':str(a.output),'commands':len(observed['cmd']),'layout':observed['desktop_layout'],'close':observed['client_close']}))
if __name__=='__main__':main()
