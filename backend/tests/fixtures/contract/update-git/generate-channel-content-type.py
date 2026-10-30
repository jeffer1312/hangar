"""Captura validação do canal e a causa de falhas do encoder original."""
import argparse,asyncio,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-channel-special-') as scratch:
 os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(scratch);sys.path.insert(0,str(ref/'backend'))
 from fastapi import FastAPI
 from fastapi.exceptions import RequestValidationError
 from fastapi.exception_handlers import request_validation_exception_handler
 import httpx
 from app import update_channel as channel
 app=FastAPI();validation={};effects=[]
 app.include_router(channel.router)
 app.dependency_overrides[channel.require_auth]=lambda:None
 app.dependency_overrides[channel.require_owner]=lambda:None
 channel._checkout_branch=lambda:'main'
 channel._git_request=lambda *args,**kwargs:subprocess.CompletedProcess(args,0,'fixture\trefs/heads/beta\n','')
 channel._write_env=lambda branch,last:effects.append(dict(branch=branch,last=last))
 channel.atualizar.estado_para_tela=lambda:{}
 channel.atualizar._tomar_a_vez=lambda:True
 channel.atualizar._soltar_a_vez=lambda:None
 channel.diag.registrar=lambda *args,**kwargs:None
 async def capture():
  rows=[]
  async with httpx.AsyncClient(transport=httpx.ASGITransport(app,raise_app_exceptions=False),base_url='http://fixture') as client:
   for content_type in [None,'application/json','application/json; charset=utf-8','application/vnd.fixture+json','Application/JSON','text/plain','text/json','application/octet-stream','']:
    effects.clear();channel.settings.update_branch='';channel.settings.update_last_branch=''
    response=await client.put('/api/update-channel',content='{"branch":"beta"}',headers={} if content_type is None else {'content-type':content_type})
    rows.append(dict(content_type=content_type,status=response.status_code,text=response.text,effects=list(effects)))
  return rows
 out.write_text(json.dumps(dict(source_commit=commit,command='uv run --project backend --with httpx python backend/tests/fixtures/contract/update-git/generate-channel-content-type.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/channel-content-type.json',cases=asyncio.run(capture())),ensure_ascii=True,indent=2)+'\n')
