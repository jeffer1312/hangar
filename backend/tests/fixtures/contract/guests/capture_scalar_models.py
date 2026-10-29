"""Captura coerção escalar dos modelos HTTP originais a partir dos bytes JSON."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch:
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_AUTH_TOKEN='owner')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app.guest_users_api import GuestSettings
    from app.sync import VaultPutBody
    app=FastAPI()
    for name,model in [('integer',VaultPutBody),('boolean',GuestSettings)]:
        def endpoint(body):return body.model_dump()
        endpoint.__annotations__={'body':model};app.add_api_route('/'+name,endpoint,methods=['POST'])
    client=TestClient(app,raise_server_exceptions=False)
    numeric=['0','1','0.0','1.0','-0.0','1.0000000000000001','100000000000000000001.0',
             '-100000000000000000001.0','100000000000000000001','1e20','1e400','-1e400','NaN','Infinity','-Infinity','1e-400']
    numeric += ['1'*4300,'1'*4301,'-'+'1'*4300]
    numeric += ['-9223372036854774784.0','9223372036854774784.0','9223372036854775808.0','-9223372036854775808.0','-9223372036854777856.0','1.5','-1.5']
    strings=['+-1','-+1','++1','--1','+-1.0','-+1.0','++1.0','--1.0','1.0','1.00','1_0.0',
             '100000000000000000001.0','\u00a01\u00a0','\u20031\u2003','\x1c1\x1c','\t1\n',' 1 ',
             '1e3','١','１','+1','-1','1__0','1_.0','.0','1.','true','false',' True ']
    strings += ['0'*5000+'1'*20,'-'+'0'*5000+'1'*20,'+'+'0'*5000+'1'*20]
    strings += ['-'+'1'*4299,'0'*5000,'-'+'0'*5000,'1_'*2500+'1']
    strings += ['1'*4300,'1'*4301,'-'+'1'*4300,'+'+'1'*4300]
    rows=[]
    for kind in ['integer','boolean']:
        for raw in numeric+[json.dumps(value) for value in strings]+['null','true','false','[]','{}']:
            body='{"base_rev":'+raw+',"enc_blob":null}' if kind=='integer' else '{"root":"fixture","sees_owner":'+raw+'}'
            response=client.post('/'+kind,content=body,headers={'Content-Type':'application/json'})
            try:payload=response.json()
            except ValueError:payload=response.text
            rows.append({'kind':kind,'raw_value':raw,'raw_body':body,'status':response.status_code,'response':payload})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_scalar_models.py','rows':rows}
    (HERE/'scalar-models.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(f'{len(rows)} escalares capturados em modelos HTTP')
