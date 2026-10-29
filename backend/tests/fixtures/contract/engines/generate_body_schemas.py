"""Prende os modelos HTTP originais e Content-Type sem iniciar o backend."""
import ast
import json
import subprocess
from pathlib import Path
from types import SimpleNamespace
from fastapi import FastAPI
from fastapi.testclient import TestClient
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel, ConfigDict, Field

ROOT = Path(__file__).resolve().parents[5]
namespace = dict(BaseModel=BaseModel, ConfigDict=ConfigDict, Field=Field,
                 agentes_sync=SimpleNamespace(ALVOS=("pi", "kimi", "codex")))
classes = ("_StrictBody", "EngineProbeBody", "CliproxyManagementBody", "EngineModelBody", "SyncBody", "ApelidoBody")
for source in ("api.py", "credenciais.py"):
    nodes = [node for node in ast.parse((ROOT / "backend/app" / source).read_text()).body
             if isinstance(node, ast.ClassDef) and node.name in classes]
    exec(compile(ast.Module(body=nodes, type_ignores=[]), source, "exec"), namespace)

cases = []
for schema in classes[1:]:
    app = FastAPI()
    app.add_middleware(CORSMiddleware,allow_origins=["*"],allow_methods=["*"],allow_headers=["*"],
                       expose_headers=["ETag","x-hangar-css-width","x-hangar-css-height"])
    async def endpoint(body):
        return body.model_dump()
    endpoint.__annotations__["body"] = namespace[schema]
    app.post("/body")(endpoint)
    client = TestClient(app, raise_server_exceptions=False)
    payloads = [b"", b"null", b"[]", b"{}", b'{"extra":1}', b'{"id":"chave:motor"}',
                b'{"model":"k3"}', b'{"model":7}', b'{"management_key":false}',
                b'{"id":"x","alvos":null}', b'{"id":"x","alvos":[1]}',
                b'{"id":"x","alvos":[]}', b'{"id":"x","apelido":null}',
                b'{"nome":7}', b'{"base_url":false}', b'{"model":NaN}',
                b'{"extra":"\\ud800"}', b'{"management_key":"' + b'x' * 513 + b'"}']
    for index, body in enumerate(payloads):
        response = client.post("/body", content=body, headers={"content-type": "application/json", "origin":"https://fixture.invalid"})
        cases.append({"schema": schema, "name": f"body-{index}", "hex": body.hex(), "content_type": "application/json",
                      "status": response.status_code, "headers":dict(response.headers), "body": response.json() if response.headers.get("content-type", "").startswith("application/json") else response.text})
    for content_type in (None, "text/plain", "application/octet-stream", "text/json", "application/json", "application/problem+json", "application/json; charset=ascii"):
        body = b'{}'
        response = client.post("/body", content=body, headers={"origin":"https://fixture.invalid", **({} if content_type is None else {"content-type": content_type})})
        cases.append({"schema": schema, "name": "content-type", "hex": body.hex(), "content_type": content_type,
                      "status": response.status_code, "headers":dict(response.headers), "body": response.json() if response.headers.get("content-type", "").startswith("application/json") else response.text})
Path(__file__).with_name("body-schemas.json").write_text(json.dumps({
    "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    "command": "uv run --with httpx python tests/fixtures/contract/engines/generate_body_schemas.py", "cases": cases,
}, ensure_ascii=True, indent=2) + "\n")
