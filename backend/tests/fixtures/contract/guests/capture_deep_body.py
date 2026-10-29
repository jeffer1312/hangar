"""Captura o limite do parser HTTP com modelo original e campo extra ignorado."""
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
    from app.sync import LoginBody
    app = FastAPI()
    @app.post('/probe')
    def probe(body: LoginBody):
        return {'user': body.user, 'auth_hash': body.auth_hash}
    client = TestClient(app, raise_server_exceptions=False)
    rows = []
    for depth in [100,150,500,900,950,975,980,985,990,995,1000,1500,2000,5000,10000]:
        for shape in ['array','object']:
            opening, closing = ('[',']') if shape == 'array' else ('{"x":','}')
            raw = '{"user":"fixture","auth_hash":"fixture","extra":' + opening * depth + '0' + closing * depth + '}'
            response = client.post('/probe', content=raw, headers={'Content-Type':'application/json'})
            try: body = response.json()
            except ValueError: body = response.text
            rows.append({'depth':depth,'shape':shape,'status':response.status_code,'response':body})
    result = {'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
              'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_deep_body.py',
              'model':'app.sync.LoginBody','endpoint':'isolado, com campo extra ignorado','rows':rows}
    (HERE/'deep-body.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
    print(json.dumps(rows))
