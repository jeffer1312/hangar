"""Captura PUT real de motores antes de qualquer I/O, com corpo vazio, inválido ou não objeto."""
import ast
import json
from pathlib import Path
import subprocess
from fastapi import FastAPI, HTTPException, Request
from fastapi.testclient import TestClient
from fastapi.middleware.cors import CORSMiddleware

ROOT=Path(__file__).resolve().parents[5]
node=next(n for n in ast.parse((ROOT/"backend/app/api.py").read_text()).body if isinstance(n,ast.AsyncFunctionDef) and n.name=="put_engine")
node.decorator_list=[]
namespace=dict(Request=Request,HTTPException=HTTPException,erro=lambda code,msg,**params:dict(code=code,msg=msg,params=params))
exec(compile(ast.Module(body=[node],type_ignores=[]),"original-put-engine","exec"),namespace)
app=FastAPI()
app.add_middleware(CORSMiddleware,allow_origins=["*"],allow_methods=["*"],allow_headers=["*"],expose_headers=["ETag","x-hangar-css-width","x-hangar-css-height"])
app.put("/api/engines/{nome}")(namespace["put_engine"])
client=TestClient(app,raise_server_exceptions=False)
cases=[]
for name,body in [("empty",b""),("space",b" "),("whitespace",b"\r\n\t"),("null",b"null"),("array",b"[]")]:
    for content_type in (None,"application/json","text/plain","application/problem+json"):
        for origin in (None,"https://fixture.invalid"):
            headers={}
            if content_type is not None:headers["content-type"]=content_type
            if origin is not None:headers["origin"]=origin
            response=client.put("/api/engines/fixture",content=body,headers=headers)
            cases.append(dict(name=name,hex=body.hex(),content_type=content_type,origin=origin,
                status=response.status_code,headers=dict(response.headers),body=response.text))
Path(__file__).with_name("raw-body.json").write_text(json.dumps(dict(
    source_commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
    command="cd backend && uv run --with httpx python tests/fixtures/contract/engines/generate_raw_body.py",
    cases=cases),ensure_ascii=True,indent=2)+"\n")
print(f"{len(cases)} respostas originais capturadas")
