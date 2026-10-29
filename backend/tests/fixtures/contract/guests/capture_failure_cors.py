"""Captura CORS de erros tratados e exceções nas rotas originais do hub."""
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_SYNC='1',CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret',CP_LAN_BIND_IP='127.0.0.1')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from fastapi import FastAPI
    from fastapi.middleware.cors import CORSMiddleware
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_admin_router);app.include_router(sync.sync_router)
    app.add_middleware(CORSMiddleware,allow_origins=['*'],allow_methods=['*'],allow_headers=['*'],expose_headers=['ETag','x-hangar-css-width','x-hangar-css-height'])
    client=TestClient(app,raise_server_exceptions=False)
    valid={'user':'ana','salt':'eHh4eHh4eHh4eHh4eHh4eA==','verifier_salt':'eHh4eHh4eHh4eHh4eHh4eA==','auth_verifier':sync.make_verifier('fixture',b'x'*16),'enc_blob':{'text':'\ud800'},'rev':0}
    specs=[('validation','POST','/api/sync/setup',{'user':'\ud800'},None),('serialization','GET','/api/sync/vault',None,json.dumps(valid)),
           ('read','GET','/api/sync/status',None,'invalid'),('success','GET','/api/sync/status',None,None),
           ('handled','POST','/api/sync/login',{'user':'ana','auth_hash':'bad'},None)]
    rows=[]
    with patch('app.sync.time.time',lambda:2000000000):
        cookie='cp_sync='+sync.sign_session('ana')
        for name,method,path,body,disk in specs:
            (root/'vault.json').unlink(missing_ok=True)
            if disk is not None:(root/'vault.json').write_text(disk)
            client.cookies.clear()
            response=client.request(method,path,content=json.dumps(body),headers={'Content-Type':'application/json','Authorization':'Bearer owner','Cookie':cookie,'Origin':'https://client.invalid'})
            try:payload=response.json()
            except ValueError:payload=response.text
            rows.append({'name':name,'method':method,'path':path,'body':body,'disk':disk,'status':response.status_code,'response':payload,
                         'cors':{k:v for k,v in response.headers.items() if k.startswith('access-control-')}})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_failure_cors.py','cookie':cookie,'rows':rows}
    (HERE/'failure-cors.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print([(r['name'],r['status'],r['cors']) for r in rows])
