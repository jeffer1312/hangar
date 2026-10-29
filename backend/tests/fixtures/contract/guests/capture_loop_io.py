"""Captura falha real de persistência nos handlers originais; permissões Unix temporárias."""
import ast
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from fastapi import FastAPI,HTTPException
from fastapi.testclient import TestClient
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel,ConfigDict,Field
sys.path.insert(0,str(Path.cwd()))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch);os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='fixture')
    from app import loop as loop_mod
    from app.mensagens import erro
    source=Path.cwd()/'app/api.py';nodes=[]
    names={'loop_create','loop_stop','loop_resolve'}
    for node in ast.parse(source.read_text()).body:
        if isinstance(node,ast.ClassDef) and node.name in ['_StrictBody','LoopCreate','LoopResolve']:nodes.append(node)
        if isinstance(node,ast.FunctionDef) and node.name in names:node.decorator_list=[];nodes.append(node)
    queued=[];pushed=[]
    class Queue:
        def __init__(self,name):pass
        def append(self,text,delivered=False):queued.append({'text':text,'delivered':delivered});return {'id':'entry-1'}
    info=SimpleNamespace(name='s',provider='claude',git_dir=None,cwd=None,jsonl=None)
    ns={'BaseModel':BaseModel,'ConfigDict':ConfigDict,'Field':Field,'HTTPException':HTTPException,'loop_mod':loop_mod,'erro':erro,
        'registry':SimpleNamespace(list=lambda:[info]),'automations_enabled':lambda:True,'branch_of':lambda _:None,'PromptQueue':Queue,
        'push':SimpleNamespace(notify_loop=lambda *args:pushed.append(args))}
    exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),ns)
    app=FastAPI()
    for method,tail,name in [('POST','loop','loop_create'),('DELETE','loop','loop_stop'),('POST','loop/resolve','loop_resolve')]:app.add_api_route('/api/sessions/{name}/'+tail,ns[name],methods=[method])
    app.add_middleware(CORSMiddleware,allow_origins=['*'],allow_methods=['*'],allow_headers=['*'],expose_headers=['ETag','x-hangar-css-width','x-hangar-css-height'])
    rows=[]
    with TestClient(app,raise_server_exceptions=False) as client,patch.object(loop_mod,'time',SimpleNamespace(time=lambda:1000.0)):
        for method,tail,body,status in [('POST','loop',{'goal':'novo'},'done'),('DELETE','loop',None,'running'),('POST','loop/resolve',{'accept':True},'done_claimed'),('POST','loop/resolve',{'accept':False},'done_claimed')]:
            link=loop_mod.LoopLink('s');old=loop_mod.new_loop('anterior',None,10,True);old['status']=status;link.set(old)
            before=link.path.read_bytes();queued.clear();pushed.clear();link.path.parent.chmod(0o555)
            try:response=client.request(method,'/api/sessions/s/'+tail,json=body,headers={'Origin':'https://fixture.example'})
            finally:link.path.parent.chmod(0o755)
            try:payload=response.json()
            except ValueError:payload=response.text
            rows.append({'method':method,'tail':tail,'body':body,'initial':old,'status':response.status_code,'response':payload,
                         'unchanged':before==link.path.read_bytes(),'queued':list(queued),'pushed':list(pushed),'temporary_exists':link.path.with_suffix('.tmp').exists(),
                         'cors':response.headers.get('access-control-allow-origin')})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_io.py','isolation':'Handlers originais por AST; fila/registro/avisos simulados; persistência real em pasta Unix sem escrita, anterior preservado. Sem auth ou backend real.','rows':rows}
    (Path(__file__).resolve().parent/'loop-io.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} falhas reais de sidecar capturadas')
