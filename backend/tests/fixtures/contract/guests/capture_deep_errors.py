"""Captura erros tipados profundos antes e depois do encoder FastAPI."""
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_SYNC='1',CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync
    app=FastAPI();app.include_router(sync.sync_router)
    from fastapi.exceptions import RequestValidationError
    from fastapi.exception_handlers import request_validation_exception_handler
    recorded=[]
    @app.exception_handler(RequestValidationError)
    async def capture_before_encoder(request,error):
        recorded.append(json.dumps({'detail':error.errors()},ensure_ascii=False,separators=(',',':'),allow_nan=False))
        return await request_validation_exception_handler(request,error)
    client=TestClient(app,raise_server_exceptions=False)
    valid={'user':'fixture','salt':'eHh4eHh4eHh4eHh4eHh4eA==','verifier_salt':'eHh4eHh4eHh4eHh4eHh4eA==','auth_verifier':sync.make_verifier('fixture',b'x'*16),'rev':0,'enc_blob':None}
    rows=[]
    with patch('app.sync.time.time',lambda:2000000000):
        cookie=sync.sign_session('fixture')
        for operation in ['invalid','invalid_vault']:
            for depth in [100,150,500,900,950,975,980,985,990,995,1000,1500,2000,5000,10000]:
                for shape in ['array','object']:
                    opening,closing=('[',']') if shape=='array' else ('{"x":','}')
                    nested=opening*depth+'0'+closing*depth
                    disk=json.dumps(valid)
                    if operation=='vault':disk=disk.replace('"enc_blob": null','"enc_blob": {"nested":'+nested+'}')
                    (root/'vault.json').write_text(disk);sync._FAILS.clear();client.cookies.clear()
                    if operation=='invalid':
                        raw='{"user":'+nested+',"auth_hash":"fixture"}'
                        response=client.post('/api/sync/login',content=raw,headers={'Content-Type':'application/json'})
                    else:
                        raw='{"enc_blob":{"nested":'+nested+'},"base_rev":"bad"}'
                        response=client.put('/api/sync/vault',content=raw,headers={'Content-Type':'application/json','Cookie':'cp_sync='+cookie})
                    before_encoder=recorded[-1]
                    rows.append({'before_encoder':before_encoder,'operation':operation,'depth':depth,'shape':shape,'status':response.status_code,'raw_response':response.text,'disk_unchanged':(root/'vault.json').read_text()==disk})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_deep_errors.py','vault':valid,'cookie':cookie,'rows':rows}
    (HERE/'deep-errors.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} chamadas profundas capturadas')
