"""Captura HEAD e Allow antes das dependências de autenticação."""
import os,sys,tempfile,json,subprocess
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp,CP_AUTH_TOKEN='owner');os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from fastapi import FastAPI
 from fastapi.testclient import TestClient
 from app import peers_api,alcance,update_channel
 app=FastAPI();app.include_router(peers_api.peers_router);app.include_router(alcance.alcance_router);app.include_router(update_channel.router)
 rows=[]
 for path in ['/api/peers','/api/peers/app-list','/api/peers/ping','/api/peers/prova','/api/peers/identificador','/api/peers/descobrir','/api/peers/a/token','/api/alcance','/api/alcance/pareamento','/api/update-channel']:
  for authenticated in [False,True]:
   r=TestClient(app).head(path,headers={'authorization':'Bearer owner'} if authenticated else {});rows.append(dict(path=path,authenticated=authenticated,status=r.status_code,allow=r.headers.get('allow'),body=r.text))
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-head.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=False,indent=2)+'\n');print(rows)
