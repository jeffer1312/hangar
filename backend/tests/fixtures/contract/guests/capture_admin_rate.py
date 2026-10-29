"""Captura contador do dono e precedência de parser/modelo nas rotas de convidados."""
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    os.environ.pop('HANGAR_AUTH_CONTEXT',None)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import auth,guest_users,guest_users_api,guest_user_gate,sync
    guest_users._path_override=root/'guests.json'
    app=FastAPI();app.include_router(guest_users_api.router);app.include_router(sync.sync_admin_router)
    app.add_middleware(guest_user_gate.GuestUserGate)
    with patch('secrets.token_hex',lambda n:'ab'*n),patch('secrets.token_urlsafe',lambda n:'guest-token'):
        guest_users.create('Ana',scratch,False,True)
    specs=[('get', [('GET','/api/me',None,'bad')]*8),
           ('malformed', [('POST','/api/guests','{','bad')]*8),
           ('invalid_model', [('POST','/api/guests','{}','bad')]*8),
           ('shared_admin', [('GET','/api/me',None,'bad')]*4+[('GET','/api/sync/setup',None,'bad')]*4),
           ('guest_forbidden', [('POST','/api/guests','{}','guest-token')]*8)]
    rows=[]
    for name,steps in specs:
        auth._fails.clear()
        client=TestClient(app,client=('198.51.100.7',12345),raise_server_exceptions=False)
        outputs=[]
        for method,path,raw,token in steps+[('GET','/api/me',None,'owner')]:
            headers={'Authorization':'Bearer '+token}
            if raw is not None:headers['Content-Type']='application/json'
            response=client.request(method,path,content=raw,headers=headers)
            outputs.append({'method':method,'path':path,'raw':raw,'token':token,'status':response.status_code,
                            'response':response.json(),'retry_after':response.headers.get('retry-after')})
        rows.append({'name':name,'steps':outputs})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_admin_rate.py',
            'profile':'require_auth original sem contexto compartilhado; paridade do contador compartilhado é conferida separadamente', 'rows':rows}
    (HERE/'admin-rate.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(f'{sum(len(row["steps"]) for row in rows)} respostas capturadas')
