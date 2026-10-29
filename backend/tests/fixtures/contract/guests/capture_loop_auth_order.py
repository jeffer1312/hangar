"""Captura ordem parse/auth/dependência/schema com modelos e require_auth originais."""
import ast
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
    transfer={'busy':False,'calls':0}
    def guard():
        transfer['calls']+=1
        if transfer['busy']:raise HTTPException(409,detail='fixture_transfer_busy')
    def endpoint(body):return body.model_dump()
    endpoint.__annotations__={'body':ns['LoopCreate']}
    app=FastAPI();app.add_api_route('/loop',endpoint,methods=['POST'],dependencies=[Depends(require_auth),Depends(guard)])
    rows=[]
    with TestClient(app,raise_server_exceptions=False) as client:
        for token in ['owner','invalid']:
            for busy in [False,True]:
                for media in ['application/json','text/plain',None]:
                    for body in [b'{',b'\xff',b'',b'null',b'[]',b'{}',b'{"goal":"g"}']:
                        reset_backoff()
                        transfer.update(busy=busy,calls=0)
                        headers={'Authorization':'Bearer '+token}
                        if media:headers['Content-Type']=media
                        response=client.post('/loop',content=body,headers=headers)
                        try:value=response.json()
                        except ValueError:value=response.text
                        rows.append({'token':token,'busy':busy,'content_type':media,'body_hex':body.hex(),'status':response.status_code,'response':value,'transfer_calls':transfer['calls']})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_auth_order.py','isolation':'Modelo LoopCreate e require_auth originais; dependência de transferência simulada só para medir precedência, sem afirmar contrato do mecanismo de transferência; reset_backoff entre casos exclui contador desta captura.','rows':rows}
    (Path(__file__).resolve().parent/'loop-auth-order.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} ordens de entrada capturadas')
