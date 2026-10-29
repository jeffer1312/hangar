"""Captura registros HTTP originais dos loops; handlers sem efeito e sem autenticação."""
import ast
import json
import subprocess
from pathlib import Path
from fastapi import FastAPI
from fastapi.testclient import TestClient
HERE=Path(__file__).resolve().parent
source=Path.cwd()/'app/api.py'
app=FastAPI()
registrations=[]
for node in ast.parse(source.read_text()).body:
    if not isinstance(node,(ast.FunctionDef,ast.AsyncFunctionDef)):continue
    for decorator in node.decorator_list:
        if not isinstance(decorator,ast.Call) or not isinstance(decorator.func,ast.Attribute) or not decorator.args:continue
        path=decorator.args[0]
        if not isinstance(path,ast.Constant) or not isinstance(path.value,str):continue
        if path.value not in ['/api/sessions/{name}/loop','/api/sessions/{name}/loop/refine','/api/sessions/{name}/loop/resolve','/api/sessions/{name}/then']:continue
        method=decorator.func.attr.upper()
        def endpoint(name:str):return {'matched':True}
        app.add_api_route(path.value,endpoint,methods=[method])
        registrations.append([path.value,method])
rows=[]
with TestClient(app,follow_redirects=False) as client:
    for tail in ['loop','loop/refine','loop/resolve','then','loop/missing']:
        for path in [f'/api/sessions/fixture/{tail}',f'/api/sessions/fixture/{tail}/',f'/api/sessions/fixture%2Fother/{tail}',f'/api/%73essions/fixture/{tail}']:
            for method in ['GET','POST','PUT','DELETE','PATCH','HEAD','OPTIONS']:
                request_path=path+'?x=1%202&x=3'
                response=client.request(method,request_path)
                rows.append({'path':request_path,'method':method,'status':response.status_code,'body':response.text,'headers':{k:v for k,v in response.headers.items() if k in ['allow','location','content-type','content-length']}})
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_loop_routing.py','isolation':'Registro path/método extraído dos decorators originais; handler sem efeitos, autenticação/validação fora desta captura. Matched não é prova do efeito da rota.','routes':registrations,'rows':rows}
(HERE/'loop-routing.json').write_text(json.dumps(result,ensure_ascii=True,indent=2)+'\n')
print(f'{len(rows)} contratos de roteamento capturados')
