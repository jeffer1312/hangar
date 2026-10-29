"""Confere serialização dos erros do handler original após a validação do modelo."""
import ast
import json
from pathlib import Path
import subprocess
from fastapi import FastAPI, HTTPException
from fastapi.testclient import TestClient
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel, ConfigDict

ROOT=Path(__file__).resolve().parents[5]
helper=Path(__file__).with_name("generate_engine_http.py")
nodes=ast.parse(helper.read_text()).body
nodes=[node for node in nodes if not (isinstance(node,ast.Expr) and isinstance(node.value,ast.Call) and isinstance(node.value.func,ast.Attribute) and node.value.func.attr == "run")]
namespace={"__file__":str(helper)}
exec(compile(ast.Module(body=nodes,type_ignores=[]),str(helper),"exec"),namespace)
source=ast.parse((ROOT/"backend/app/api.py").read_text())
models=[node for node in source.body if isinstance(node,ast.ClassDef) and node.name in ("_StrictBody","EngineModelBody")]
model_namespace=dict(BaseModel=BaseModel,ConfigDict=ConfigDict)
exec(compile(ast.Module(body=models,type_ignores=[]),"original-model","exec"),model_namespace)
app=FastAPI()
app.add_middleware(CORSMiddleware,allow_origins=["*"],allow_methods=["*"],allow_headers=["*"],
                   expose_headers=["ETag","x-hangar-css-width","x-hangar-css-height"])
async def endpoint(body):
    result=(await namespace["scenario"]("serialization",model=body.model,effort=body.effort))["result"]
    if result["status"]!=200:raise HTTPException(result["status"],result["detail"])
    return result["body"]
endpoint.__annotations__["body"]=model_namespace["EngineModelBody"]
app.post("/model")(endpoint)
client=TestClient(app,raise_server_exceptions=False)
cases=[]
for model in ("\ud800","x\udfff","NaN","missing"):
    request=json.dumps({"model":model}).encode()
    response=client.post("/model",content=request,headers={"content-type":"application/json","origin":"https://fixture.invalid"})
    cases.append(dict(hex=request.hex(),status=response.status_code,headers=dict(response.headers),body=response.json() if response.headers.get("content-type","").startswith("application/json") else response.text))
Path(__file__).with_name("model-serialization.json").write_text(json.dumps(dict(
 source_commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
 command="uv run --with httpx python tests/fixtures/contract/engines/generate_model_serialization.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
