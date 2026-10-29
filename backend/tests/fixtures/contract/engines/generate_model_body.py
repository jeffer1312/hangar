"""Captura a borda FastAPI com o modelo original, sem importar o backend."""
import ast
import json
import subprocess
from pathlib import Path
from fastapi import FastAPI
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict

root = Path(__file__).resolve().parents[5]
source = ast.parse((root / 'backend/app/api.py').read_text())
nodes = [node for node in source.body if isinstance(node, ast.ClassDef) and node.name in ('_StrictBody', 'EngineModelBody')]
namespace = {'BaseModel': BaseModel, 'ConfigDict': ConfigDict}
exec(compile(ast.Module(body=nodes, type_ignores=[]), 'original-engine-body', 'exec'), namespace)
app = FastAPI()
async def endpoint(body):
    return body.model_dump()
endpoint.__annotations__['body'] = namespace['EngineModelBody']
app.post('/model')(endpoint)
client = TestClient(app, raise_server_exceptions=False)
cases = []
values = ['', 'null', '[]', '{}', '{"effort":"high"}', '{"model":null}', '{"model":7}', '{"model":true}', '{"model":[]}', '{"model":"k3","effort":7}', '{"model":"k3","extra":true}', '{"model":false,"effort":[],"extra":1}', '{"model":""}', '{"model":"k3"}', '{"model":"k3","effort":null}', '{"model":"k3","effort":"high"}', '{"model":NaN}', '{"model":Infinity}', '{"model":"\\ud800"}', '{"model":"k3","extra":"\\ud800"}', '{"model":"k3",}', '{"model":}']
for index, value in enumerate(values):
    cases.append((f'body-{index}', value.encode()))
for encoding in ('utf-8-sig', 'utf-16', 'utf-16-le', 'utf-16-be', 'utf-32', 'utf-32-le', 'utf-32-be'):
    cases.append((encoding, '{"model":"k3"}'.encode(encoding)))
cases.append(('invalid-utf8', b'{"model":"\xff"}'))
output = []
for name, body in cases:
    response = client.post('/model', content=body, headers={'content-type': 'application/json'})
    output.append({'name': name, 'hex': body.hex(), 'status': response.status_code, 'body': response.json() if response.headers.get('content-type', '').startswith('application/json') else response.text})
Path(__file__).with_name('model-body.json').write_text(json.dumps({'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'command': 'uv run --with httpx python tests/fixtures/contract/engines/generate_model_body.py', 'cases': output}, ensure_ascii=True, indent=2) + '\n')
