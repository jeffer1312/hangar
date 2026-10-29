"""Captura a lista e as mutações de convidados em cofres antigos."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_SYNC='1',CP_SYNC_BOOTSTRAP='bootstrap',
                      CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret',CP_LAN_BIND_IP='127.0.0.1')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_admin_router);app.include_router(sync.sync_router)
    client=TestClient(app,client=('127.0.0.1',12345),raise_server_exceptions=False)
    valid={'user':'ana','salt':'eHh4eHh4eHh4eHh4eHh4eA==','verifier_salt':'eHh4eHh4eHh4eHh4eHh4eA==',
           'auth_verifier':sync.make_verifier('fixture',b'x'*16),'enc_blob':None,'rev':0}
    documents=[valid]
    documents += [valid|{'guests':value} for value in [None,{},[],False,0,0.0,'',True,1,'x',['x']]]
    documents += [valid|{'guests':{'guest':value}} for value in [None,{},[],False,0,0.0,'',True,1,'x',{'admin_blob':None}]]
    documents += [valid|{'guests':{'guest':{'rev':value,'admin_blob':None}}} for value in [None,False,True,-1,0,1,2**64,10**100,0.5,'1',[],{}]]
    rows=[]
    with patch('app.sync.time.time',lambda:2000000000),patch('secrets.token_bytes',lambda n:b'x'*n):
        cookie=sync.sign_session('ana')
        import base64
        blob={'iv':base64.b64encode(b'x'*12).decode(),'data':base64.b64encode(b'x'*16).decode()}
        body={'user':'guest','salt':base64.b64encode(b'x'*16).decode(),'auth_hash':base64.b64encode(b'x'*32).decode(),'enc_blob':blob,'admin_blob':blob}
        specs=[('GET','/api/sync/guests',None),('POST','/api/sync/guests',body),('POST','/api/sync/guests/guest/delete',None)]
        for document in documents:
            cases=[]
            for method,path,body in specs:
                sync.save_vault(document);client.cookies.clear();sync._FAILS.clear()
                response=client.request(method,path,json=body,headers={'Authorization':'Bearer owner','Cookie':'cp_sync='+cookie})
                try:payload=response.json()
                except ValueError:payload=response.text
                cases.append({'method':method,'path':path,'body':body,'status':response.status_code,'response':payload,
                              'disk':json.loads((root/'vault.json').read_text())})
            rows.append({'initial':document,'cases':cases})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_vault_guest_shapes.py','cookie':cookie,'rows':rows}
    (HERE/'vault-guest-shapes.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{sum(len(row["cases"]) for row in rows)} operações com formas de cofre capturadas')
