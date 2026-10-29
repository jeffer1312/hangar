"""Corpos malformados em rotas que não declaram corpo no FastAPI."""
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',
                      CP_SYNC='1',CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret',CP_LAN_BIND_IP='127.0.0.1')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import guest_users,guest_users_api,sync,runtime_config
    guest_users._path_override=root/'guests.json';runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(guest_users_api.router);app.include_router(sync.sync_router);app.include_router(sync.sync_admin_router)
    client=TestClient(app,raise_server_exceptions=False);rows=[]
    for method,path in [('GET','/api/me'),('POST','/api/guests/missing/delete'),('GET','/api/sync/setup'),
                        ('POST','/api/sync/setup/disable'),('GET','/api/sync/vault'),('GET','/api/sync/guests'),
                        ('GET','/api/sync/prelogin?user=owner')]:
        for token in ['owner','unknown']:
            for file in [root/'guests.json',root/'runtime-config.json',root/'vault.json']:
                file.unlink(missing_ok=True)
            guest_users._reset();runtime_config._cache=None
            response=client.request(method,path,content=b'{',headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
            try:body=response.json()
            except ValueError:body=response.text
            rows.append({'method':method,'path':path,'token':token,'raw':'{','status':response.status_code,'response':body})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_ignored_body.py','rows':rows}
    (HERE/'ignored-body.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} requisições capturadas')
