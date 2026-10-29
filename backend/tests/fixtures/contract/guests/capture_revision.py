"""Captura revisões sem limite e coerções na rota do cofre isolada."""
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
    root = Path(scratch)
    os.environ.update(HOME=scratch, USERPROFILE=scratch, CLAUDE_CONFIG_DIR=scratch,
                      CP_SYNC='1', CP_SYNC_DATA=str(root/'vault.json'),
                      CP_SYNC_SESSION_SECRET='fixture-secret', CP_LAN_BIND_IP='127.0.0.1')
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync, runtime_config
    runtime_config._backend_config_base = lambda: root
    http = json.loads((HERE/'http.json').read_text())
    seed = next(row['vault'] for row in http['rows'] if row['name']=='registrar')
    app = FastAPI()
    app.include_router(sync.sync_router)
    client = TestClient(app, raise_server_exceptions=False)
    rows = []
    cases = [(2**64-1, 2**64-1), (10**80, str(10**80)), (1, True), (1, '1.0'),
             (1000, 1000.0), (1000, '1e3'), (1000, '1_000'), (-2, -2)]
    with patch('app.sync.time.time', lambda: 2000000000):
        cookie = sync.sign_session('owner', sync._generation(seed))
        for revision, supplied in cases:
            initial = dict(seed, rev=revision)
            (root/'vault.json').write_text(json.dumps(initial))
            body = {'enc_blob':None, 'base_rev':supplied}
            response = client.put('/api/sync/vault',json=body,headers={'Cookie':'cp_sync='+cookie})
            rows.append({'initial':initial,'body':body,'status':response.status_code,
                         'response':response.json(),'vault':json.loads((root/'vault.json').read_text())})
    (HERE/'revision.json').write_text(json.dumps({'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_revision.py',
        'cookie':cookie,'rows':rows},ensure_ascii=False,indent=2)+'\n')
    print(f'{len(rows)} revisões Python capturadas')
