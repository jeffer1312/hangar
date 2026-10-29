"""Captura ativação, desativação e cofres inválidos na API Python isolada."""
import base64
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',
                      CP_SYNC='0',CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_router);app.include_router(sync.sync_admin_router)
    client=TestClient(app,raise_server_exceptions=False)
    payload={'user':'owner','salt':base64.b64encode(b's'*16).decode(),'auth_hash':base64.b64encode(b'a'*32).decode(),
             'enc_blob':{'iv':base64.b64encode(b'i'*12).decode(),'data':base64.b64encode(b'd'*16).decode()}}
    rows=[]
    def state():
        return {key:path.read_text() if path.exists() else None for key,path in [('vault',root/'vault.json'),('config',root/'runtime-config.json')]}
    def call(name,method,path,body=None,owner=True):
        initial=state()
        response=client.request(method,path,json=body,headers={'Authorization':'Bearer owner'} if owner else {})
        try: value=response.json()
        except ValueError:value=response.text
        rows.append({'name':name,'method':method,'path':path,'body':body,'owner':owner,'initial':initial,
                     'status':response.status_code,'response':value,'after':state(),'set_cookie':response.headers.get('set-cookie')})
    with patch('secrets.token_bytes',lambda n:b'x'*n),patch('app.sync.time.time',lambda:2000000000):
        call('disabled','GET','/api/sync/status',owner=False)
        call('setup_unauthorized','GET','/api/sync/setup',owner=False)
        call('initial','GET','/api/sync/setup')
        call('missing_account','POST','/api/sync/setup',{})
        call('create','POST','/api/sync/setup',payload)
        call('enabled','GET','/api/sync/status',owner=False)
        call('disable','POST','/api/sync/setup/disable')
        call('disabled_vault','GET','/api/sync/vault',owner=False)
        call('duplicate','POST','/api/sync/setup',payload)
        call('reactivate','POST','/api/sync/setup',{})
        valid=json.loads((root/'vault.json').read_text())
        for index,content in enumerate(['{','{}','null',json.dumps(dict(valid,rev=True)),json.dumps(dict(valid,rev=10**80)),json.dumps(dict(valid,enc_blob={})),json.dumps(dict(valid,salt='!'))]):
            (root/'vault.json').write_text(content)
            call(f'invalid_{index}','GET','/api/sync/setup')
    (HERE/'setup.json').write_text(json.dumps({'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_setup.py','rows':rows},ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} cenários setup Python capturados')
