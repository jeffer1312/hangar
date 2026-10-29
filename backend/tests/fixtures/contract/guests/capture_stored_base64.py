"""Captura o decodificador permissivo das credenciais que já estão no cofre."""
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
                      CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_router);client=TestClient(app,raise_server_exceptions=False)
    canonical=base64.b64encode(b'x'*16).decode()
    cases=[canonical,canonical[:-3]+'B==',canonical+'=',canonical.rstrip('='),canonical+' \n',
           '!'+canonical,'='+canonical,canonical+'AAAA',canonical[:2]+'='+canonical[2:],
           '', '====','A','AA','AAA','AAAA','AA==junk','é'+canonical,None,17,[],{}]
    rows=[]
    def result(response):
        try:body=response.json()
        except ValueError:body=response.text
        return {'status':response.status_code,'response':body}
    with patch('app.sync.time.time',lambda:2000000000):
        for salt in cases:
            try:
                decoded=base64.b64decode(salt)
                verifier=sync.make_verifier('fixture-auth',decoded)
                parser={'accepted':True,'bytes_hex':decoded.hex()}
            except Exception as exc:
                verifier='invalid';parser={'accepted':False,'exception':type(exc).__name__}
            vault={'user':'ana','salt':'fixture-salt','verifier_salt':salt,'auth_verifier':verifier,'enc_blob':None,'rev':0}
            sync.save_vault(vault);before=(root/'vault.json').read_bytes();client.cookies.clear();sync._FAILS.clear()
            prelogin=client.get('/api/sync/prelogin',params={'user':'unknown'})
            login=client.post('/api/sync/login',json={'user':'ana','auth_hash':'fixture-auth'})
            rows.append({'salt':salt,'vault':vault,'parser':parser,'prelogin':result(prelogin),'login':result(login),
                         'set_cookie':login.headers.get('set-cookie'),'file_unchanged':before==(root/'vault.json').read_bytes()})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_stored_base64.py','rows':rows}
    (HERE/'stored-base64.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} credenciais armazenadas capturadas')
