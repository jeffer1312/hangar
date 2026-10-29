"""Caracteriza a leitura do arquivo legado e a regravação pela API, sem lifespan."""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    root=Path(scratch)
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import guest_users, guest_users_api
    guest_users._path_override=root/'guests.json'
    app=FastAPI();app.include_router(guest_users_api.router)
    client=TestClient(app,raise_server_exceptions=False)
    guest={'id':'old','name':'Ana','root':scratch,'token_hash':hashlib.sha256(b'old-token').hexdigest()}
    documents=[{}, {'sessions':{}}, {'guests':[]}, {'guests':[guest]},
               {'guests':[guest,dict(guest,name='Bia')]},
               {'guests':[guest],'sessions':[['session',{'guest':'old','life':None}]]},
               {'guests':[dict(guest,extra=True)]}, {'guests':None}, {'guests':[], 'sessions':None}]
    rows=[]
    for document in documents:
        guest_users._path_override.write_text(json.dumps(document));guest_users._reset()
        body={'name':'Nova','root':scratch}
        with patch('secrets.token_hex',lambda n:'ab'*n), patch('secrets.token_urlsafe',lambda n:'new-token'):
            response=client.post('/api/guests',json=body,headers={'Authorization':'Bearer owner'})
        rows.append({'initial':document,'body':body,'status':response.status_code,'response':response.json(),
                     'disk':json.loads(guest_users._path_override.read_text())})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_storage.py','rows':rows}
    (HERE/'storage.json').write_text(json.dumps(result,ensure_ascii=False,indent=2).replace(scratch,'⟦ROOT⟧')+'\n')
    print(f'{len(rows)} arquivos Python capturados')
