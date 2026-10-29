"""Captura tipos JSON especiais nas rotas irmãs, sem acessar dados da instalação."""
import json,os,subprocess,sys,tempfile
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 root=Path(tmp);os.environ.update(HOME=tmp,USERPROFILE=tmp,CP_AUTH_TOKEN='owner');os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from fastapi import FastAPI
 from fastapi.testclient import TestClient
 from app import peers,peers_api
 from app.config import settings
 peers._PEERS_FILE=root/'peers.json';peers_api._ENV_ARQUIVO=root/'.env';settings.auth_token='owner'
 app=FastAPI();app.include_router(peers_api.peers_router);client=TestClient(app,raise_server_exceptions=False);rows=[]
 initial={'remote':{'base_url':'https://peer.invalid','token':'fixture-token'}}
 for token in ['NaN','Infinity','-Infinity','"\\ud800"']:
  for field in ['id','label','address','token','lan.url','lan.id','rev','extra','enabled','identificador']:
   peers._PEERS_FILE.write_text(json.dumps(initial,indent=2)+'\n');peers_api._ENV_ARQUIVO.write_text('');settings.server_id=''
   rev=client.get('/api/peers/app-list',headers={'authorization':'Bearer owner'}).json()['rev']
   entry={'id':'machine','label':'Machine','address':'https://peer.invalid','token':'fixture-token','lan':{'url':'http://192.0.2.1:8765','id':'remote'}}
   body={'rev':rev,'servers':[entry]};path='/api/peers/app-list'
   if field in ['enabled','identificador']:
    path='/api/peers/remote/enabled' if field=='enabled' else '/api/peers/identificador';body={field:'__TOKEN__'}
   elif field in ['rev','extra']:body[field]='__TOKEN__'
   elif field.startswith('lan.'):entry['lan'][field[4:]]='__TOKEN__'
   else:entry[field]='__TOKEN__'
   wire=json.dumps(body).replace('"__TOKEN__"',token)
   r=client.put(path,content=wire,headers={'authorization':'Bearer owner','content-type':'application/json'})
   rows.append(dict(name=field+'-'+token,path=path,wire=wire,status=r.status_code,response=r.text,initial=json.dumps(initial,indent=2)+'\n',effect=peers._PEERS_FILE.read_text(),env=peers_api._ENV_ARQUIVO.read_text()))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-peer-json-siblings.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=True,indent=2)+'\n')
print(f'{len(rows)} contratos de tipos nas rotas irmãs')
