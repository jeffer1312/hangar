"""Caracteriza campos extras grandes sem guardar megabytes regeneráveis na fixture."""
import hashlib,json,os,subprocess,sys,tempfile
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp,CP_AUTH_TOKEN='owner');os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from fastapi import FastAPI
 from fastapi.testclient import TestClient
 from app import peers,peers_api
 from app.config import settings
 peers._PEERS_FILE=Path(tmp)/'peers.json';settings.auth_token='owner'
 app=FastAPI();app.include_router(peers_api.peers_router);client=TestClient(app)
 cases=[]
 for extra in [65537,17*1024*1024]:
  body={'id':'remote','base_url':'https://peer.invalid','token':'fixture-token','extra':'x'*extra}
  wire=json.dumps(body).encode();r=client.post('/api/peers',content=wire,headers={'authorization':'Bearer owner','content-type':'application/json'})
  cases.append(dict(extra_bytes=extra,wire_bytes=len(wire),wire_sha256=hashlib.sha256(wire).hexdigest(),status=r.status_code,response=r.json(),effect=json.loads(peers._PEERS_FILE.read_text()) if peers._PEERS_FILE.exists() else None))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-large-body.py <checkout-Python> <fixture>',cases=cases),ensure_ascii=False,indent=2)+'\n');print(f'{len(cases)} corpos grandes capturados')
