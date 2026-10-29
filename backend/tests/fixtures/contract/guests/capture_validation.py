"""Executa os modelos publicados com a borda FastAPI, sem lifespan ou arquivos reais."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch)
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app.guest_users_api import NewGuest, GuestSettings
    from app.sync import SetupBody, RegisterBody, LoginBody, VaultPutBody, GuestAccountBody
    app = FastAPI()
    models = [(NewGuest,'/api/guests'), (GuestSettings,'/api/guests/fixture'),
              (SetupBody,'/api/sync/setup'), (RegisterBody,'/api/sync/register'),
              (LoginBody,'/api/sync/login'), (VaultPutBody,'/api/sync/vault'),
              (GuestAccountBody,'/api/sync/guests')]
    rows = []
    for model, path in models:
        def endpoint(body):
            return body.model_dump()
        endpoint.__annotations__ = {'body':model}
        app.add_api_route('/' + model.__name__, endpoint, methods=['POST'])
    client = TestClient(app)
    for model, path in models:
        cases = [None, {}, [], 4, '', {'root':'', 'name':'', 'sees_owner':None, 'owner_sees':'maybe'},
                 {'user':1,'salt':False,'auth_hash':[],'bootstrap':{},'enc_blob':[], 'base_rev':'wat'},
                 {'root':'fixture','name':'Ana','sees_owner':'true','owner_sees':0},
                 {'base_rev':True,'enc_blob':None}, {'base_rev':'1.0','enc_blob':{}},
                 {'base_rev':10**80,'enc_blob':{}}, {'user':None,'salt':None,'auth_hash':None},
                 {'user':' ','salt':'!','auth_hash':'!','enc_blob':{'iv':'x','data':'x'},'admin_blob':None},
                 {'user':'ana','salt':'','auth_hash':'','enc_blob':{},'admin_blob':{},'extra':True}]
        for index, body in enumerate(cases):
            response=client.post('/'+model.__name__,json=body)
            rows.append({'model':model.__name__,'index':index,'path':path,
                         'method':'PUT' if model is VaultPutBody else 'POST',
                         'body':body,'status':response.status_code,'response':response.json()})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_validation.py',
            'rows':rows}
    (HERE/'validation.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(f'{len(rows)} validações Python capturadas')
