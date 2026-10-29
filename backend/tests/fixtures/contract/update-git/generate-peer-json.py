"""Captura JSON aceito e tipos especiais, com todos os efeitos em diretório temporário."""
import json,os,subprocess,sys,tempfile
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp,CP_AUTH_TOKEN='owner');os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from fastapi import FastAPI
 from fastapi.testclient import TestClient
 from app import peers,peers_api
 from app.config import settings
 peers._PEERS_FILE=Path(tmp)/'peers.json';settings.auth_token='owner'
 app=FastAPI();app.include_router(peers_api.peers_router);client=TestClient(app,raise_server_exceptions=False)
 cases=[];base='"id":"remote","base_url":"https://peer.invalid","token":"fixture-token"'
 inputs=[('extra-depth150','{'+base+',"extra":'+'['*150+'0'+']'*150+'}'),('extra-int4300','{'+base+',"extra":'+'9'*4300+'}'),('extra-int4301','{'+base+',"extra":'+'9'*4301+'}')]
 for token in ['NaN','Infinity','-Infinity','"\\ud800"']:
  inputs.append(('extra-'+token,'{'+base+',"extra":'+token+'}'))
  inputs.append(('token-'+token,'{'+base.replace('"fixture-token"',token)+'}'))
 for name,wire in inputs:
  peers._PEERS_FILE.write_text('{}')
  r=client.post('/api/peers',content=wire.encode(),headers={'authorization':'Bearer owner','content-type':'application/json'})
  cases.append(dict(name=name,wire=wire,status=r.status_code,content_type=r.headers.get('content-type'),response=r.text,effect=peers._PEERS_FILE.read_text()))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-peer-json.py <checkout-Python> <fixture>',cases=cases),ensure_ascii=True,indent=2)+'\n');print(f'{len(cases)} contratos JSON de peers capturados')
