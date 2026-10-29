"""Captura a coerção de caminhos do Settings original sem dados pessoais."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch
HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE.parents[3]))
with tempfile.TemporaryDirectory() as scratch, patch.dict(os.environ,{'HOME':scratch,'USERPROFILE':scratch},clear=True):
    from app.config import Settings
    from fastapi import FastAPI
    from fastapi.testclient import TestClient
    from app import sync,runtime_config
    app=FastAPI();app.include_router(sync.sync_router)
    client=TestClient(app,raise_server_exceptions=False)
    previous=Path.cwd();os.chdir(scratch)
    rows=[]
    for value in ['', '.', './vault.json', 'folder//vault.json', 'folder/../vault.json', '~/.hangar/vault.json']:
        setting=Settings(_env_file=None,sync_data=value)
        with patch.object(sync,'settings',setting),patch.object(runtime_config,'get',lambda key:True):
            response=client.get('/api/sync/status')
        try:body=response.json()
        except ValueError:body=response.text
        rows.append({'input':value,'path':str(setting.sync_data),'status':response.status_code,'response':body})
    os.chdir(previous)
    result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
            'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_settings_paths.py',
            'platform':sys.platform,'rows':rows}
    (HERE/'settings-paths.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(json.dumps(rows))
