"""Captura ordem parse/auth/dependência/schema com modelos e require_auth originais."""
import ast
import asyncio
import threading
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from fastapi import Depends,FastAPI,HTTPException
from fastapi.testclient import TestClient
from pydantic import BaseModel,ConfigDict,Field
sys.path.insert(0,str(Path.cwd()))
source=Path.cwd()/'app/api.py'
ns={'BaseModel':BaseModel,'ConfigDict':ConfigDict,'Field':Field}
nodes=[n for n in ast.parse(source.read_text()).body if isinstance(n,ast.ClassDef) and n.name in ['_StrictBody','LoopCreate']]
exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),'exec'),ns)
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_LAN_BIND_IP='127.0.0.1')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from app.auth import require_auth, reset_backoff
    from app import conversation_transfer
    conversation_transfer.session_life=lambda name: None
    guard_ns={'asyncio':asyncio,'HTTPException':HTTPException}
    guard_node=next(n for n in ast.parse(source.read_text()).body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_transfer_guard')
    exec(compile(ast.Module(body=[guard_node],type_ignores=[]),str(source),'exec'),guard_ns)
    guard=guard_ns['_transfer_guard']
    def endpoint(name,body):return body.model_dump()
    endpoint.__annotations__={'name':str,'body':ns['LoopCreate']}
    app=FastAPI();app.add_api_route('/api/sessions/{name}/loop',endpoint,methods=['POST'],dependencies=[Depends(require_auth),Depends(guard)])
    rows=[]
    with TestClient(app,raise_server_exceptions=False) as client:
        for token in ['owner','invalid']:
            for busy in [False,True]:
                for media in ['application/json','text/plain',None]:
                    for body in [b'{',b'\xff',b'',b'null',b'[]',b'{}',b'{"goal":"g"}']:
                        reset_backoff()
                        conversation_transfer._operation_locks.clear()
                        if busy:
                            held=threading.Lock();held.acquire()
                            conversation_transfer._operation_locks['fixture']=held
                        headers={'Authorization':'Bearer '+token}
                        if media:headers['Content-Type']=media
                        response=client.post('/api/sessions/fixture/loop',content=body,headers=headers)
                        try:value=response.json()
                        except ValueError:value=response.text
                        rows.append({'token':token,'busy':busy,'content_type':media,'body_hex':body.hex(),'status':response.status_code,'response':value})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_transfer_order.py','isolation':'Modelo LoopCreate, require_auth, _transfer_guard e session_ingress originais. Lock real de operação adquirido no caso busy, sem executar transferência; HOME e configuração temporários; vida da sessão ausente injetada para não consultar tmux. reset_backoff entre casos exclui contador desta captura.','rows':rows}
    (Path(__file__).resolve().parent/'loop-transfer-order.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} ordens de entrada capturadas')
