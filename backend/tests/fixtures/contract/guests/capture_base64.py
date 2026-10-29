"""Captura a aceitação de base64 pela rota real, incluindo bits não canônicos."""
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner',CP_SYNC='1',
                      CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_BOOTSTRAP='bootstrap',CP_SYNC_SESSION_SECRET='fixture-secret')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_router);app.include_router(sync.sync_admin_router);client=TestClient(app,raise_server_exceptions=False)
    auth=base64.b64encode(b'x'*32).decode();salt=base64.b64encode(b'x'*16).decode()
    cases=[('canonical',auth,salt),('auth_pad_bits',auth[:-2]+'h=',salt),('salt_pad_bits',auth,salt[:-3]+'B=='),
           ('extra_padding',auth+'=',salt),('missing_padding',auth.rstrip('='),salt),('space',auth+' ',salt),
           ('newline',auth+'\n',salt),('unicode','é'+auth[1:],salt)]
    rows=[]
    with patch('secrets.token_bytes',lambda n:b'x'*n),patch('app.sync.time.time',lambda:2000000000):
        for path,name,hash_value,salt_value in [(path,*case) for path in ['/api/sync/register','/api/sync/setup'] for case in cases]:
            (root/'vault.json').unlink(missing_ok=True);client.cookies.clear()
            body={'user':'ana','auth_hash':hash_value,'salt':salt_value,'bootstrap':'bootstrap'}
            if path.endswith('/setup'):body.pop('bootstrap')
            response=client.post(path,json=body,headers={'Authorization':'Bearer owner'})
            rows.append({'path':path,'name':name,'body':body,'status':response.status_code,'response':response.json(),
                         'vault':json.loads((root/'vault.json').read_text()) if (root/'vault.json').exists() else None})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_base64.py','rows':rows}
    (HERE/'base64.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print([(r['path'],r['name'],r['status']) for r in rows])
