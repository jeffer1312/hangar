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
 app=FastAPI();app.include_router(peers_api.peers_router);app.dependency_overrides[peers_api.require_auth]=lambda:None
 async def capture():
  rows=[]
  values=['NaN','Infinity','123456789012345678901234567890','true','[1]','{"x":1}','"\\ud800"','"açúcar"']
  cases=[]
  for field in ['base_url','token','web_url']:
   for value in values:
    entry={'base_url':'https://a.invalid','token':'fixture-token'};entry.pop(field,None)
    initial='{"a":{'+json.dumps(entry)[1:-1]+',"'+field+'":'+value+'}}'
    for path in ['/api/peers','/api/peers/a/token']:cases.append((field+'-'+value,initial,path))
  for value in values:cases.append(('app-'+value,'{"a":{"app":{"id":"a","address":"https://a.invalid","extra":'+value+'}}}','/api/peers/app-list'))
  async with httpx.AsyncClient(transport=httpx.ASGITransport(app,raise_app_exceptions=False),base_url='http://fixture') as client:
   for label,initial,path in cases:
     method='GET';body=None
     peers._PEERS_FILE.write_text(initial);stale=peers._PEERS_FILE.with_name('peers.json.fixture.tmp');stale.write_text('synthetic')
     response=await client.request(method,path,json=body) if body is not None else await client.request(method,path)
     effect=peers._PEERS_FILE.read_bytes()
     rows.append(dict(name=label,method=method,path=path,body=body,initial=initial,status=response.status_code,text=response.text,effect_sha256=hashlib.sha256(effect).hexdigest(),effect_len=len(effect),remaining_tmp=sorted(v.name for v in Path(scratch).glob('peers.json.*.tmp'))))
     for tmp in Path(scratch).glob('peers.json.*.tmp'):tmp.unlink()
  return rows
 out.write_text(json.dumps(dict(source_commit=commit,command='uv run --project backend --with httpx python backend/tests/fixtures/contract/peers/generate-exposed-fields.py --reference <checkout-Python> --output backend/tests/fixtures/contract/peers/exposed-fields.json',cases=asyncio.run(capture())),ensure_ascii=True,indent=2)+'\n')
 print('56 casos de campos expostos capturados')
