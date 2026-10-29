"""Captura precedência das credenciais do login próprio nas rotas públicas."""
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import guest_users,guest_users_api,guest_user_gate
    guest_users._path_override=Path(scratch)/'guests.json'
    with patch('secrets.token_hex',lambda n:'ab'*n),patch('secrets.token_urlsafe',lambda n:'guest-token'):
        guest_users.create('Ana',scratch,False,True)
    app=FastAPI();app.include_router(guest_users_api.router);app.add_middleware(guest_user_gate.GuestUserGate)
    rows=[]
    cases=[('http','GET','/api/me',{'Authorization':'Bearer guest-token'}),
           ('http','GET','/api/me?token=guest-token',{}),
           ('http','GET','/api/me',{'Cookie':'cp_token=guest-token'}),
           ('https','GET','/api/me',{'Cookie':'cp_token=guest-token'}),
           ('https','GET','/api/me',{'Cookie':'__Host-cp_token=guest-token'}),
           ('http','GET','/api/me?token=guest-token',{'Authorization':'Bearer owner'}),
           ('http','GET','/api/me?token=owner',{'Authorization':'Bearer guest-token'}),
           ('http','GET','/api/me?token=guest-token',{'Authorization':'Bearer wrong'}),
           ('http','GET','/api/me?token=wrong',{'Cookie':'cp_token=guest-token'}),
           ('http','GET','/api/me',{'Cookie':'__Host-cp_token=wrong; cp_token=guest-token'}),
           ('http','POST','/api/guests',{'Cookie':'cp_token=guest-token'}),
           ('https','POST','/api/guests',{'Cookie':'__Host-cp_token=guest-token'}),
           ('http','POST','/api/guests',{'Authorization':'Bearer guest-token'})]
    for scheme,method,path,headers in cases:
        with TestClient(app,base_url=f'{scheme}://testserver') as client:
            response=client.request(method,path,headers=headers,json={})
        rows.append({'scheme':scheme,'method':method,'path':path,'headers':headers,'status':response.status_code,'response':response.json()})
    (HERE/'credentials.json').write_text(json.dumps({'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_credentials.py','rows':rows},ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} credenciais Python capturadas')
