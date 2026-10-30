"""Captura o livro de peers em disco e os efeitos das rotas, com HOME isolado."""
import argparse,asyncio,hashlib,json,os,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args();ref=args.reference.resolve();out=args.output.resolve();commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='hangar-peer-file-http-') as scratch:
 os.environ.update(HOME=scratch,USERPROFILE=scratch);os.chdir(scratch);sys.path.insert(0,str(ref/'backend'))
 from fastapi import FastAPI
 import httpx
 from app import peers,peers_api
 peers._PEERS_FILE=Path(scratch)/'peers.json';peers_api._ENV_ARQUIVO=Path(scratch)/'.env';peers_api._ENV_ARQUIVO.write_text('')
 app_instance=FastAPI();app_instance.include_router(peers_api.peers_router);app_instance.dependency_overrides[peers_api.require_auth]=lambda:None
 async def capture():
  rows=[]
  deep='['*10000+'0'+']'*10000
  common='"base_url":"https://a.invalid","token":"fixture-token"'
  cases=[]
  for field in ['base_url','token','web_url']:
   entry={'base_url':'https://a.invalid','token':'fixture-token'};entry.pop(field,None)
   initial='{"a":{'+json.dumps(entry)[1:-1]+',"'+field+'":'+deep+'}}'
   cases.append((field,initial,'POST','/api/peers',{'id':'a','base_url':'https://new.invalid','token':'fixture-new','web_url':'https://web.invalid'}))
  cases.append(('enabled','{"a":{'+common+',"enabled":'+deep+'}}','PUT','/api/peers/a/enabled',{'enabled':False}))
  for app in ['',',"app":{"id":"app-a","label":"A","address":"https://a.invalid","token":"app-token"}']:
   cases.append(('remove-app' if app else 'remove','{"a":{'+common+',"extra":'+deep+app+'}}','DELETE','/api/peers/a',None))
  app='{"id":"app-a","label":"A","address":"https://a.invalid","token":"app-token","pos":'+deep+'}'
  for method in ['GET','PUT']:
   cases.append(('app-pos','{"a":{"app":'+app+'}}',method,'/api/peers/app-list',{} if method=='PUT' else None))
  cases.append(('app-id','{"a":{"app":{"id":'+'['*1500+'0'+']'*1500+'}}}','PUT','/api/peers/app-list',{}))
  cases.append(('app-surrogate','{"a":{"app":{"id":"a","extra":"\\ud800"}}}','PUT','/api/peers/app-list',{'rev':'fixture','servers':[]}))
  cases.append(('identity-deep','{}','PUT','/api/peers/identificador',None))
  async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app_instance,raise_app_exceptions=False),base_url='http://fixture') as client:
   for label,initial,method,path,body in cases:
     peers._PEERS_FILE.write_text(initial);stale=peers._PEERS_FILE.with_name('peers.json.fixture.tmp');stale.write_text('synthetic')
     if label in ('app-pos','app-id') and method=='PUT':
      original_limit=sys.getrecursionlimit();sys.setrecursionlimit(20000)
      try:body={'rev':peers.ler_lista_app()[0],'servers':[]}
      finally:sys.setrecursionlimit(original_limit)
     wire='{"identificador":'+'['*10000+'0'+']'*10000+'}' if label=='identity-deep' else None
     response=await client.request(method,path,content=wire,headers={'content-type':'application/json'}) if wire is not None else (await client.request(method,path,json=body) if body is not None else await client.request(method,path))
     effect=peers._PEERS_FILE.read_bytes()
     rows.append(dict(name=label,method=method,path=path,body=body,wire_body=wire,initial=initial,status=response.status_code,text=response.text,effect_sha256=hashlib.sha256(effect).hexdigest(),effect_len=len(effect),remaining_tmp=sorted(v.name for v in Path(scratch).glob('peers.json.*.tmp'))))
     for tmp in Path(scratch).glob('peers.json.*.tmp'):tmp.unlink()
  return rows
 out.write_text(json.dumps(dict(source_commit=commit,command='uv run --project backend --with httpx python backend/tests/fixtures/contract/peers/generate-file-replacements.py --reference <checkout-Python> --output backend/tests/fixtures/contract/peers/file-replacements.json',cases=asyncio.run(capture())),ensure_ascii=True,indent=2)+'\n')
 print('11 casos de substituição e validação profunda capturados')
