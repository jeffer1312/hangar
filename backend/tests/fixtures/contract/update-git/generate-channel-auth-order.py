"""Captura validação do canal e a causa de falhas do encoder original."""
import argparse,asyncio,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-channel-special-') as scratch:
 os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(scratch);sys.path.insert(0,str(ref/'backend'))
 from fastapi import FastAPI, HTTPException
 from fastapi.exceptions import RequestValidationError
 from fastapi.exception_handlers import request_validation_exception_handler
 import httpx
 from app import update_channel as channel
 app=FastAPI();validation={};effects=[]
 app.include_router(channel.router)
 auth_calls=[]
 def denied():auth_calls.append("auth");raise HTTPException(401,detail="fixture unauthorized")
 app.dependency_overrides[channel.require_auth]=denied
 app.dependency_overrides[channel.require_owner]=lambda:None
 channel._checkout_branch=lambda:'main'
 channel._git_request=lambda *args,**kwargs:subprocess.CompletedProcess(args,0,'fixture\trefs/heads/beta\n','')
 channel._write_env=lambda branch,last:effects.append(dict(branch=branch,last=last))
 channel.atualizar.estado_para_tela=lambda:{}
 channel.atualizar._tomar_a_vez=lambda:True
 channel.atualizar._soltar_a_vez=lambda:None
 channel.diag.registrar=lambda *args,**kwargs:None
 @app.exception_handler(RequestValidationError)
 async def record(request,error):
  validation['json']=json.dumps({'detail':error.errors()},ensure_ascii=True)
  try:return await request_validation_exception_handler(request,error)
  except Exception as exc:
   validation['exception']=type(exc).__name__;raise
 async def capture():
  rows=[]
  async with httpx.AsyncClient(transport=httpx.ASGITransport(app,raise_app_exceptions=False),base_url='http://fixture') as client:
   for name,body in [('syntax','{'),('empty',''),('null','null'),('wrong-field','{"branch":2}'),('valid','{"branch":"beta"}')]:
    auth_calls.clear();response=await client.put('/api/update-channel',content=body,headers={'content-type':'application/json'})
    rows.append(dict(name=name,body=body,status=response.status_code,text=response.text,auth_calls=list(auth_calls)))
  return rows
 out.write_text(json.dumps(dict(source_commit=commit,command='uv run --project backend --with httpx python backend/tests/fixtures/contract/update-git/generate-channel-auth-order.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/channel-auth-order.json',cases=asyncio.run(capture())),ensure_ascii=True,indent=2)+'\n')
