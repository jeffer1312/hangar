"""Caracteriza campos profundos nos modelos originais sem iniciar o backend."""
import ast
import json
import subprocess
import sys
from pathlib import Path
from fastapi import FastAPI
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field, ValidationError

ROOT = Path(__file__).resolve().parents[5]
namespace = dict(BaseModel=BaseModel, ConfigDict=ConfigDict, Field=Field)
for source in ("api.py", "credenciais.py"):
    nodes = [node for node in ast.parse((ROOT / "backend/app" / source).read_text()).body
             if isinstance(node, ast.ClassDef) and node.name in ("_StrictBody", "EngineModelBody", "ApelidoBody")]
    exec(compile(ast.Module(body=nodes, type_ignores=[]), source, "exec"), namespace)
cases = []
for schema in ("EngineModelBody", "ApelidoBody"):
    app = FastAPI()
    async def endpoint(body): return body.model_dump()
    endpoint.__annotations__["body"] = namespace[schema]
    app.post("/body")(endpoint)
    client = TestClient(app, raise_server_exceptions=False)
    for depth in (500, 1500, 10000):
        nested = b'[' * depth + b'0' + b']' * depth
        field = b'model' if schema == 'EngineModelBody' else b'apelido'
        for kind, raw in (
            ('extra', b'{"model":"k3","id":"x","extra":' + nested + b'}'),
            ('field', b'{"id":"x","' + field + b'":' + nested + b'}'),
        ):
            response = client.post('/body', content=raw, headers={'content-type':'application/json'})
            try:
                semantic = namespace[schema].model_validate(json.loads(raw)).model_dump()
                semantic_status = 200
            except ValidationError as error:
                errors = error.errors(include_url=False)
                for item in errors: item['loc'] = ['body', *item['loc']]
                semantic = {'detail': errors}
                semantic_status = 422
            previous_limit = sys.getrecursionlimit()
            try:
                # Só a gravação da referência semântica usa limite maior; a resposta acima é original.
                sys.setrecursionlimit(40000)
                semantic_body = json.dumps(semantic, ensure_ascii=False, separators=(',', ':'))
            finally:
                sys.setrecursionlimit(previous_limit)
            cases.append(dict(schema=schema,depth=depth,kind=kind,hex=raw.hex(),status=response.status_code,
                body=response.text,headers=dict(response.headers),semantic_status=semantic_status,
                semantic_body=semantic_body))
Path(__file__).with_name('deep-bodies.json').write_text(json.dumps(dict(
    source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
    command='uv run --with httpx python tests/fixtures/contract/engines/generate_deep_bodies.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
