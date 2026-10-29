"""Captura expiração e precedência do cookie assinado com segredo exclusivo da prova."""
import hashlib
import hmac
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
    os.environ.update(HOME=scratch,USERPROFILE=scratch,CLAUDE_CONFIG_DIR=scratch,CP_SYNC='1',CP_AUTH_TOKEN='owner',
                      CP_SYNC_DATA=str(root/'vault.json'),CP_SYNC_SESSION_SECRET='fixture-secret',CP_LAN_BIND_IP='127.0.0.1')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    runtime_config._backend_config_base=lambda:root
    app=FastAPI();app.include_router(sync.sync_router);client=TestClient(app,raise_server_exceptions=False)
    def signed(user,expiration):
        msg=f'{user}.{expiration}'
        return msg+'.'+hmac.new(b'fixture-secret',msg.encode(),hashlib.sha256).hexdigest()
    specs=[('ana',exp) for exp in ['1999999999','2000000000','2000000001','02000000001','+2000000001',str(10**80),'-1','1e100','1_000','', '2000000001:abcd','2000000001:']]
    specs.extend([('', '2000000001'),('a.b','2000000001')])
    cases=[{'name':f'exp_{i}','cookie':signed(user,exp),'method':'GET','site':None} for i,(user,exp) in enumerate(specs)]
    cases.extend({'name':f'site_{method}_{site}','cookie':signed('ana','2000000001'),'method':method,'site':site}
                 for method in ['GET','PUT'] for site in ['same-origin','same-site','cross-site','none','SAME-SITE'])
    rows=[]
    with patch('app.sync.time.time',lambda:2000000000):
        for case in cases:
            client.cookies.clear();headers={'Cookie':'cp_sync='+case['cookie']}
            if case['site'] is not None:headers['Sec-Fetch-Site']=case['site']
            body={'enc_blob':None,'base_rev':0} if case['method']=='PUT' else None
            response=client.request(case['method'],'/api/sync/vault',headers=headers,json=body)
            rows.append(case|{'path':'/api/sync/vault','body':body,'status':response.status_code,'response':response.json(),'set_cookie':response.headers.get('set-cookie')})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_cookies.py','rows':rows}
    (HERE/'cookies.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} cookies e políticas de origem capturados')
