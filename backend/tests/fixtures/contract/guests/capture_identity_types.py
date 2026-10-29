"""Caracteriza tipos estruturais malformados antes da diferença aprovada D03."""
import hashlib
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
    path=Path(scratch)/'guests.json';guest_users._path_override=path
    app=FastAPI();app.include_router(guest_users_api.router);app.add_middleware(guest_user_gate.GuestUserGate)
    client=TestClient(app,raise_server_exceptions=False)
    guest={'id':'old','name':'Ana','root':scratch,'token_hash':hashlib.sha256(b'guest-token').hexdigest()}
    rows=[]
    for field in ['id','name','root','token_hash']:
        for value in [None,0,1.5,True,[],{}]:
            initial={'guests':[dict(guest,**{field:value})],'sessions':{}}
            results=[]
            for method,route,token,body in [('GET','/api/me','guest-token',None),('POST','/api/guests','owner',{'name':'Nova','root':scratch})]:
                path.write_text(json.dumps(initial));guest_users._reset()
                with patch('secrets.token_hex',lambda n:'ab'*n),patch('secrets.token_urlsafe',lambda n:'new-token'):
                    response=client.request(method,route,json=body,headers={'Authorization':'Bearer '+token})
                try: payload=response.json()
                except ValueError:payload=response.text
                results.append({'method':method,'path':route,'token':token,'body':body,'status':response.status_code,'response':payload,'disk':json.loads(path.read_text())})
            rows.append({'field':field,'value':value,'initial':initial,'results':results})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_identity_types.py','rows':rows}
    (HERE/'identity-types.json').write_text(json.dumps(result,ensure_ascii=False,indent=2).replace(scratch,'⟦ROOT⟧')+'\n')
    print(f'{len(rows)} tipos Python capturados')
