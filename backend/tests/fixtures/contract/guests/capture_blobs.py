"""Captura o padding aceito no conteúdo cifrado pela configuração real do hub."""
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
    app=FastAPI();app.include_router(sync.sync_admin_router);client=TestClient(app,raise_server_exceptions=False)
    encoded=lambda n:base64.b64encode(b'x'*n).decode()
    cases=[('full_quantum',encoded(18)),('extra_padding_full',encoded(18)+'==='),('padding_bits',encoded(16)[:-3]+'B=='),
           ('excess_padding',encoded(16)+'='),('discontinuous',encoded(18)+'=AAAA'),('leading','='+encoded(18)),
           ('invalid_count',encoded(18)+'A'),('missing_padding',encoded(17).rstrip('='))]
    cases.extend(('suffix_'+suffix,encoded(18)+suffix) for suffix in ['A=','A==','A=A','AA=','AA==','AA===','AA=!','AAA=','AAA==','AAA=A','AA==!','AAAA=','AAAA=AAAA'])
    rows=[]
    with patch('secrets.token_bytes',lambda n:b'x'*n):
        for name,data in cases:
            (root/'vault.json').unlink(missing_ok=True)
            body={'user':'ana','salt':encoded(16),'auth_hash':encoded(32),'enc_blob':{'iv':encoded(12),'data':data}}
            response=client.post('/api/sync/setup',json=body,headers={'Authorization':'Bearer owner'})
            rows.append({'name':name,'path':'/api/sync/setup','body':body,'status':response.status_code,'response':response.json(),
                         'vault':json.loads((root/'vault.json').read_text()) if (root/'vault.json').exists() else None})
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_blobs.py','rows':rows}
    (HERE/'blobs.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    print([(r['name'],r['status']) for r in rows])
