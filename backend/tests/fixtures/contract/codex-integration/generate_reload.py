"""Captura recarga por WebSocket de loopback sem um Codex instalado."""
import ast, asyncio, contextlib, io, json, os, subprocess, sys, tempfile, time
from pathlib import Path
from types import ModuleType
from unittest.mock import patch
from websockets.asyncio.server import serve
from websockets.exceptions import ConnectionClosedError
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
ns=dict(Path=Path,json=json,os=os,time=time,sys=sys,_PREFIXO='hangar',_HOOKS_DEADLINE_S=15)
def extract(path,names):
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[node for node in ast.parse(source).body if isinstance(node,(ast.FunctionDef,ast.ClassDef)) and node.name in names]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),ns)
extract('backend/app/codex_compat.py',{'edicoes_confianca_hooks'})
extract('scripts/hangar-codex-tui',{'_confiar_hooks','_RpcError'})
compat=ModuleType('app.codex_compat');compat.edicoes_confianca_hooks=ns['edicoes_confianca_hooks']
launcher=ModuleType('app.adapters.codex.lancador');launcher.CLIENT_INFO={'name':'hangar','title':None,'version':'0.1.0'}
cases=[]
async def main():
 for name in ['empty','user','project','warnings','invalid','initialize-error','list-error','batch-error','notification','closed']:
  with tempfile.TemporaryDirectory() as raw:
   root=Path(raw);calls=[];stderr=io.StringIO();closed=[]
   hook=dict(enabled=True,trustStatus='untrusted',source='user' if name!='project' else 'project',key='fixture',currentHash='hash')
   data={'data':[]} if name in ('empty','notification') else {'data':[{'hooks':[hook]}]}
   if name=='warnings':data={'data':[{'warnings':['fixture']}]}
   if name=='invalid':data=None
   async def handler(ws):
    try:
     async for text in ws:
      req=json.loads(text);calls.append({'method':req['method'],'params':req['params']})
      if name=='closed':await ws.close();return
      if name=='notification':await ws.send(json.dumps({'jsonrpc':'2.0','method':'fixture/event','params':{}}))
      failure=(name=='initialize-error' and req['method']=='initialize') or (name=='list-error' and req['method']=='hooks/list') or (name=='batch-error' and req['method']=='config/batchWrite')
      reply={'jsonrpc':'2.0','id':req['id']}
      if failure:reply['error']={'code':-32603,'message':'fixture private'}
      else:reply['result']=data if req['method']=='hooks/list' else {}
      await ws.send(json.dumps(reply))
    except ConnectionClosedError:
     pass
    finally: closed.append(ws.close_code)
   async with serve(handler,'127.0.0.1',0) as server:
    endpoint=f'ws://127.0.0.1:{server.sockets[0].getsockname()[1]}'
    with patch.dict(sys.modules,{'app.codex_compat':compat,'app.adapters.codex.lancador':launcher}),patch.object(Path,'home',return_value=root),contextlib.redirect_stderr(stderr):
     result=await asyncio.to_thread(ns['_confiar_hooks'],endpoint,'/fixture/project',recarregar=True,codex_home=root/'account')
   cases.append(json.loads(json.dumps(dict(name=name,data=data,result=result,calls=calls,closed=closed,stderr=stderr.getvalue())).replace(raw,'$ROOT')))
asyncio.run(main())
Path(__file__).with_name('reload.json').write_text(json.dumps(dict(source_commit=COMMIT,deadline_seconds=15,open_timeout_seconds=5,close_timeout_seconds=1,
 command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_reload.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
