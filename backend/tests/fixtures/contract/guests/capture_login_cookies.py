"""Captura escaping do Set-Cookie e renovação de nomes aceitos pelo cadastro original."""
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
                      CP_SYNC_BOOTSTRAP='bootstrap',CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret',CP_LAN_BIND_IP='127.0.0.1')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_router);rows=[]
    with patch('app.sync.time.time',lambda:2000000000),patch('secrets.token_bytes',lambda n:b'x'*n):
        for user in ['ana','a.b','a b','a;b','a,b','a"b','a\\b','a/b','a:b','é','\u0080','\u00ff','\u0100','\U0001f600','a\nb','a\rb','a\tb','a\x00b','']:
            (root/'vault.json').unlink(missing_ok=True)
            client=TestClient(app,raise_server_exceptions=False)
            registered=client.post('/api/sync/register',json={'bootstrap':'bootstrap','user':user,'salt':'fixture','auth_hash':'fixture'})
            assert registered.status_code==200
            login=client.post('/api/sync/login',json={'user':user,'auth_hash':'fixture'})
            vault=client.get('/api/sync/vault')
            def result(response):
                try:body=response.json()
                except ValueError:body=response.text
                return {'status':response.status_code,'response':body,'set_cookie':response.headers.get('set-cookie')}
            rows.append({'user':user,'login':result(login),'vault':result(vault)})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_login_cookies.py','rows':rows}
    (HERE/'login-cookies.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} sequências de cookie capturadas')
