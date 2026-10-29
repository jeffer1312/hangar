"""Captura a precedência entre JSON, autenticação e modelo nos handlers originais."""
import ast, asyncio, contextvars, json, secrets, subprocess, sys, time, types
from pathlib import Path
from fastapi import Depends, FastAPI, HTTPException, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field
ROOT=Path(__file__).resolve().parents[5]
def extract(file,names,ns):
    nodes=[n for n in ast.parse((ROOT/file).read_text()).body if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef,ast.ClassDef)) and n.name in names]
    for n in nodes:n.decorator_list=[]
    exec(compile(ast.Module(body=nodes,type_ignores=[]),file,'exec'),ns)
pkg=types.ModuleType('app');sys.modules['app']=pkg
for name in ('guest_users','guest_header','shared_auth'):
    module=types.ModuleType('app.'+name);sys.modules['app.'+name]=module;setattr(pkg,name,module)
pkg.guest_users.current=contextvars.ContextVar('fixture_guest',default=None)
pkg.guest_header.guest_of=lambda request:None
pkg.shared_auth.current=lambda:None
ns=dict(asyncio=asyncio,secrets=secrets,time=time,Request=Request,HTTPException=HTTPException,
    settings=types.SimpleNamespace(auth_token='fixture-owner'),_LOOPBACK={'127.0.0.1'},COOKIE='cp_token',
    COOKIE_HOST='__Host-cp_token',_COOKIE_METODOS=('GET','HEAD'),CONNECT_PEER='192.0.2.1',registrar_acesso=lambda *a,**k:None)
extract('backend/app/mensagens.py',{'erro'},ns)
extract('backend/app/auth.py',{'require_auth','_authenticate_request','cookie_token'},ns)
auth_calls=0
async def observed_auth(request:Request):
    global auth_calls
    auth_calls+=1
    await ns['require_auth'](request)
handlers=dict(ns,BaseModel=BaseModel,ConfigDict=ConfigDict,Field=Field)
extract('backend/app/api.py',{'_StrictBody','EngineProbeBody','CliproxyManagementBody','EngineModelBody',
    'engine_modelos','engine_cliproxy_management_key','engine_model_set','put_engine'},handlers)
extract('backend/app/credenciais.py',{'ApelidoBody','SyncBody','definir_apelido','sincronizar_nos_agentes'},handlers)
app=FastAPI()
app.add_middleware(CORSMiddleware,allow_origins=['*'],allow_methods=['*'],allow_headers=['*'],expose_headers=['ETag','x-hangar-css-width','x-hangar-css-height'])
routes=[('POST','/api/engines/modelos','engine_modelos'),('PUT','/api/engines/cliproxy/management-key','engine_cliproxy_management_key'),
    ('PUT','/api/credenciais/apelido','definir_apelido'),('POST','/api/credenciais/sincronizar','sincronizar_nos_agentes'),
    ('POST','/api/sessions/{name}/engine/model','engine_model_set'),('PUT','/api/engines/{nome}','put_engine')]
for method,path,name in routes:app.add_api_route(path,handlers[name],methods=[method],dependencies=[Depends(observed_auth)])
client=TestClient(app,raise_server_exceptions=False,client=('127.0.0.1',12345));cases=[]
for method,path,handler in routes:
    path=path.replace('{name}','fixture').replace('{nome}','fixture')
    for name,body in [('empty',b''),('space',b' '),('broken',b'{'),('utf8',b'\xff'),('null',b'null'),('array',b'[]'),('field',b'{"id":false}'),('object',b'{}')]:
        for content_type in (None,'application/json','text/plain','application/problem+json'):
            headers={'origin':'https://fixture.invalid'}
            if content_type:headers['content-type']=content_type
            auth_calls=0
            response=client.request(method,path,content=body,headers=headers)
            cases.append(dict(method=method,path=path,handler=handler,name=name,hex=body.hex(),content_type=content_type,
                status=response.status_code,headers=dict(response.headers),body=response.text,auth_calls=auth_calls))
Path(__file__).with_name('auth-order.json').write_text(json.dumps(dict(source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
    command='cd backend && uv run --with httpx python tests/fixtures/contract/engines/generate_auth_order.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(f'{len(cases)} respostas originais capturadas')
