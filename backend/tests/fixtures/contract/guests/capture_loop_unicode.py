"""Executa criação/leitura original de loop com Unicode e fila isolada."""
import ast
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    from app import loop as loop_mod
    from app.mensagens import erro
    source=HERE.parents[3]/'app/api.py'
    nodes=[]
    for node in ast.parse(source.read_text()).body:
        if isinstance(node,ast.ClassDef) and node.name in ['_StrictBody','LoopCreate']:
            nodes.append(node)
        if isinstance(node,ast.FunctionDef) and node.name in ['loop_create','loop_get']:
            node.decorator_list=[];nodes.append(node)
    queued=[]
    class Queue:
        def __init__(self,name):pass
        def append(self,text,delivered=False):
            queued.append({'text':text,'delivered':delivered});return {'id':'entry-1'}
    info=SimpleNamespace(name='s',provider='claude',git_dir=None,cwd=None,jsonl=None)
    ns={'BaseModel':BaseModel,'ConfigDict':ConfigDict,'Field':Field,'HTTPException':HTTPException,
        'loop_mod':loop_mod,'erro':erro,'registry':SimpleNamespace(list=lambda:[info]),
        'automations_enabled':lambda:True,'branch_of':lambda _:None,'PromptQueue':Queue}
    exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),ns)
    app=FastAPI();app.add_api_route('/api/sessions/{name}/loop',ns['loop_create'],methods=['POST'])
    app.add_api_route('/api/sessions/{name}/loop',ns['loop_get'],methods=['GET'])
    client=TestClient(app,raise_server_exceptions=False);rows=[]
    with patch.object(loop_mod.settings,'projects_dir',root/'projects'),patch.object(loop_mod,'time',SimpleNamespace(time=lambda:1000.0)):
        for text in ['ação','passar \ud83d','\udc00','\ud83d\ude00','\ufffd','texto\x00fim']:
            loop_mod.LoopLink('s').clear();queued.clear()
            response=client.post('/api/sessions/s/loop',content=json.dumps({'goal':text}),headers={'Content-Type':'application/json'})
            read=client.get('/api/sessions/s/loop')
            def payload(r):
                try:return r.json()
                except ValueError:return r.text
            rows.append({'goal':text,'status':response.status_code,'response':payload(response),'read_status':read.status_code,'read_response':payload(read),
                         'queued':list(queued),'disk':loop_mod.LoopLink('s').get()})
            loop_mod.LoopLink('s').set(loop_mod.new_loop(text,None,10,True))
            rows[-1]['direct_disk']=loop_mod.LoopLink('s').get()
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_unicode.py','rows':rows}
    (HERE/'loop-unicode.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} criações Unicode capturadas')
