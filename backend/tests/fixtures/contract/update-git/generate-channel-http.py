"""Captura validação HTTP do canal sem git, serviço ou rede."""
import argparse,asyncio,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();ref=a.reference.resolve();out=a.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-channel-http-') as scratch:
 os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(scratch);sys.path.insert(0,str(ref/'backend'))
 from fastapi import FastAPI
 import httpx
 from app import update_channel as channel
 app=FastAPI()
 @app.put('/api/update-channel')
 def write(body:channel.ChannelRequest):return body.model_dump()
 async def capture():
  rows=[]
  async with httpx.AsyncClient(transport=httpx.ASGITransport(app),base_url='http://fixture') as client:
   for name,body in [('empty',''),('missing','{}'),('null','null'),('branch-null','{"branch":null}'),('number','{"branch":3}'),('bool','{"branch":true}'),('list','{"branch":[]}'),('object','{"branch":{}}'),('root-list','[]'),('root-string','"x"'),('invalid','{'),('accepted','{"branch":" beta ","extra":true}')]:
    response=await client.put('/api/update-channel',content=body,headers={'content-type':'application/json'});rows.append({'name':name,'body':body,'status':response.status_code,'json':response.json()})
  return rows
 rows=asyncio.run(capture());out.write_text(json.dumps({'source_commit':commit,'command':'python backend/tests/fixtures/contract/update-git/generate-channel-http.py --reference <checkout-Python> --output backend/tests/fixtures/contract/update-git/channel-http-cases.json','cases':rows},ensure_ascii=False,indent=2)+'\n');print(f'{len(rows)} casos HTTP capturados')
