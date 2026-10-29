"""Captura recusas e redirecionamentos sem executar operações de escrita ou sondas."""
import os,sys,tempfile,json,subprocess
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp,CP_AUTH_TOKEN='owner');os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from fastapi import FastAPI
 from fastapi.testclient import TestClient
 from app import peers_api,alcance,update_channel
 app=FastAPI();app.include_router(peers_api.peers_router);app.include_router(alcance.alcance_router);app.include_router(update_channel.router)
 paths={'/api/peers':['GET','POST'],'/api/peers/app-list':['GET','PUT','DELETE'],'/api/peers/ping':['GET','DELETE'],'/api/peers/a/token':['GET'],'/api/peers/a/enabled':['PUT'],'/api/peers/a':['DELETE'],'/api/peers/a/extra':[],'/api/peers/a/x/token':[],'/api/alcance':['GET'],'/api/alcance/pareamento':['GET'],'/api/update-channel':['GET','PUT']}
 rows=[];client=TestClient(app,follow_redirects=False)
 def capture(method,path):
  for authenticated in [False,True]:
   r=client.request(method,path,headers={'authorization':'Bearer owner'} if authenticated else {})
   rows.append(dict(method=method,path=path,authenticated=authenticated,status=r.status_code,allow=r.headers.get('allow'),content_type=r.headers.get('content-type'),content_length=r.headers.get('content-length'),location=r.headers.get('location','').replace('http://testserver','<BASE>'),body=r.text))
 for path,accepted in paths.items():
  for method in ['GET','POST','PUT','DELETE','PATCH','HEAD','OPTIONS','TRACE']:
   if method not in accepted:capture(method,path)
  for method in ['GET','HEAD','OPTIONS']:capture(method,path+'/?probe=1')
 for path in ['/api/peers/%70ing','/api/peers/a%2Fb/token','/api/%70eers/ping','/api/peers/%FF/token','/api/peers/ping///?probe=1']:
  for method in ['GET','HEAD']:capture(method,path)
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-routing.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=False,indent=2)+'\n')
 print(f'{len(rows)} contratos de método/caminho capturados')
