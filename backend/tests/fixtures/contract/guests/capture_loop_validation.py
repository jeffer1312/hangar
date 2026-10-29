"""Captura os modelos originais do loop pela borda FastAPI, sem carregar a aplicação."""
import ast
import json
import subprocess
from pathlib import Path
from fastapi import FastAPI
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field
HERE = Path(__file__).resolve().parent
source = HERE.parents[3] / 'app/api.py'
names = ['_StrictBody', 'LoopCreate', 'LoopResolve', 'LoopRefine', 'ThenLinkBody']
nodes = [node for node in ast.parse(source.read_text()).body if isinstance(node, ast.ClassDef) and node.name in names]
assert len(nodes) == len(names)
namespace = {'BaseModel': BaseModel, 'ConfigDict': ConfigDict, 'Field': Field}
exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), 'exec'), namespace)
app = FastAPI()
for name in names[1:]:
    def endpoint(body):
        return body.model_dump()
    endpoint.__annotations__ = {'body': namespace[name]}
    app.add_api_route('/' + name, endpoint, methods=['POST'])
rows = []
with TestClient(app) as client:
    for name in names[1:]:
        for body in [None, {}, [], 4, '', {'goal': 'g'}, {'accept': 'true'}, {'accept': 0},
                     {'goal': '', 'max_iters': 0, 'require_branch': 'maybe'},
                     {'goal': 'g', 'max_iters': 101}, {'goal': 'g', 'max_iters': '2.0', 'require_branch': 'false'},
                     {'goal': 'g', 'max_iters': 10**80}, {'goal': 'g', 'check_cmd': None},
                     {'goal': 'g', 'check_cmd': 1}, {'goal': 'g', 'extra': True}, {'goal': 'x'*2001},
                     {'target': 't', 'text': 'x'}, {'target': '', 'text': ''}, {'accept': None}, {'accept': True}]:
            response = client.post('/' + name, json=body)
            rows.append({'model': name, 'body': body, 'status': response.status_code, 'response': response.json()})
result = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
          'command': 'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_validation.py', 'rows': rows}
(HERE / 'loop-validation.json').write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n')
print(f'{len(rows)} modelos de loop capturados')
