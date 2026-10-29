"""Captura sintaxe, autenticação e MIME das escritas de peers sem executar uma escrita."""
import argparse,asyncio,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-peers-input-') as scratch:
 os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(scratch);sys.path.insert(0,str(ref/'backend'))
 from fastapi import FastAPI,HTTPException
 from fastapi.exceptions import RequestValidationError
 from fastapi.exception_handlers import request_validation_exception_handler
 import httpx
 from app import peers_api,peers
 peers._PEERS_FILE=Path(scratch)/'peers.json';peers._PEERS_FILE.write_text('{}');peers_api._ENV_ARQUIVO=Path(scratch)/'.env';peers_api._ENV_ARQUIVO.write_text('')
 app=FastAPI();app.include_router(peers_api.peers_router);auth_calls=[];authorized=False
 def auth():
  auth_calls.append('auth')
  if not authorized:raise HTTPException(401,detail='fixture unauthorized')
 app.dependency_overrides[peers_api.require_auth]=auth
 validation={}
 @app.exception_handler(RequestValidationError)
 async def record_validation(request,error):
  validation['json']=json.dumps({'detail':error.errors()},ensure_ascii=True)
  try:return await request_validation_exception_handler(request,error)
  except Exception as exc:validation['exception']=type(exc).__name__;raise
 async def capture():
  global authorized
  rows=[]
  cases=[(True,'application/json',body) for body in ['0','false','"other"','[1]','NaN','Infinity','-Infinity','"\\ud800"','['*1500+'0'+']'*1500]]
  async with httpx.AsyncClient(transport=httpx.ASGITransport(app,raise_app_exceptions=False),base_url='http://fixture') as client:
   for method,path in [('POST','/api/peers'),('PUT','/api/peers/app-list'),('PUT','/api/peers/a/enabled'),('PUT','/api/peers/identificador')]:
    for authorized,content_type,body in cases:
     auth_calls.clear();validation.clear();response=await client.request(method,path,content=body,headers={} if content_type is None else {'content-type':content_type})
     rows.append(dict(method=method,path=path,authorized=authorized,content_type=content_type,body=body,status=response.status_code,text=response.text,auth_calls=list(auth_calls),peers=peers._PEERS_FILE.read_text(),env=peers_api._ENV_ARQUIVO.read_text(),validation=dict(validation)))
  return rows
 out.write_text(json.dumps(dict(source_commit=commit,command='uv run --project backend --with httpx python backend/tests/fixtures/contract/peers/generate-models.py --reference <checkout-Python> --output backend/tests/fixtures/contract/peers/models.json',cases=asyncio.run(capture())),ensure_ascii=True,indent=2)+'\n')
 print('36 casos de modelos de peers capturados')
