"""Executa cache e invalidação dos handlers Python com provedor e disco isolados."""
import ast
import asyncio
import copy
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

ROOT=Path(__file__).resolve().parents[5]
source=ROOT/"backend/app/api.py"
nodes=[node for node in ast.parse(source.read_text()).body if isinstance(node,ast.AsyncFunctionDef)
       and node.name in ("_engine_models","put_engine","delete_engine")]
for node in nodes:
    node.decorator_list=[]
    for argument in node.args.args:argument.annotation=None
class Failure(Exception):
    def __init__(self,status_code,detail):self.status,self.detail=status_code,detail
class Request:
    async def json(self):return {}
async def main():
    state={"generation":0,"clock":0,"calls":0}
    original={"base_url":"https://provider.invalid","api_key":"fixture-key","model":"v0"}
    entries={"motor":dict(original)}
    def probe(*_):
        state["calls"]+=1
        return [{"id":f"v{state['generation']}", "context_length": None, "vision": None}]
    def save(name,body):entries[name]=body
    def remove(name):return entries.pop(name,None) is not None
    namespace=dict(asyncio=asyncio,HTTPException=Failure,time=SimpleNamespace(monotonic=lambda:state["clock"]),
        _ENGINE_MODELS_TTL=300,_engine_models_cache={},
        engines=SimpleNamespace(listar=lambda:copy.deepcopy(entries),salvar=save,remover=remove),
        engine_probe=SimpleNamespace(listar_modelos=probe),cliproxy=SimpleNamespace(redact=lambda message,key:message.replace(key,"***")),
        runtime_config=SimpleNamespace(mascarar=lambda key:"masked"),_motores_para_cliente=lambda:copy.deepcopy(entries),
        agentes_sync=SimpleNamespace(remover=lambda _:{}),_log=SimpleNamespace(warning=lambda *_:None),
        erro=lambda code,msg,**params:dict(code=code,msg=msg,params=params))
    exec(compile(ast.Module(body=nodes,type_ignores=[]),str(source),"exec"),namespace)
    cases=[]
    async def read(name,fresh=False):
        try:result={"models":await namespace["_engine_models"]("motor",fresco=fresh)}
        except Failure as error:result={"status":error.status,"detail":error.detail}
        cases.append(dict(name=name,result=result,calls=state["calls"]))
    await read("first")
    state["generation"]=1
    await read("cached")
    await namespace["put_engine"]("motor",Request())
    await read("same-put-invalidates")
    state["generation"]=2
    await namespace["delete_engine"]("motor")
    entries["motor"]=dict(original)
    await read("delete-recreate-invalidates")
    entries.clear()
    await read("external-delete-keeps-cache")
    state["clock"]=300
    await read("expired-missing")
    entries["motor"]=dict(original)
    state["generation"]=3
    await read("fresh",True)
    Path(__file__).with_name("catalog-cache.json").write_text(json.dumps(dict(
        source_commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
        command="uv run python tests/fixtures/contract/engines/generate_catalog_cache.py",cases=cases),ensure_ascii=True,indent=2)+"\n")
asyncio.run(main())
