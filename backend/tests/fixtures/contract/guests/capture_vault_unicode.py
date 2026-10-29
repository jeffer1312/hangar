"""Captura caracteres que o modelo aceita no cofre e a serialização HTTP posterior."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_SYNC='1',CP_SYNC_BOOTSTRAP='fixture-bootstrap',CP_AUTH_TOKEN='owner',
                      CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret',CP_LAN_BIND_IP='127.0.0.1')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_router);client=TestClient(app,raise_server_exceptions=False)
    rows=[]
    for blob in [{'x':'normal'},{'x':'\ud800'},{'\udfff':'x'},{'x':['\ud800']},{'x':float('nan')},{'x':float('inf')}]:
        (root/'vault.json').unlink(missing_ok=True)
        assert client.post('/api/sync/register',json={'bootstrap':'fixture-bootstrap','user':'ana','salt':'fixture','auth_hash':'fixture'}).status_code==200
        cookie=sync.sign_session('ana')
        raw=json.dumps({'enc_blob':blob,'base_rev':0})
        def result(response):
            try:body=response.json()
            except ValueError:body=response.text
            return {'status':response.status_code,'response':body}
        put=client.put('/api/sync/vault',content=raw,headers={'Cookie':'cp_sync='+cookie,'Content-Type':'application/json'})
        get=client.get('/api/sync/vault',headers={'Cookie':'cp_sync='+cookie})
        stale=client.put('/api/sync/vault',json={'enc_blob':None,'base_rev':0},headers={'Cookie':'cp_sync='+cookie})
        disk=json.loads((root/'vault.json').read_text())
        rows.append({'raw_body':raw,'put':result(put),'get':result(get),'stale':result(stale),
                     'disk_blob':disk['enc_blob'],'disk_rev':disk['rev']})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_vault_unicode.py','rows':rows}
    (HERE/'vault-unicode.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} sequências de gravação e leitura capturadas')
