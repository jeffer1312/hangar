"""Captura o CORS original e sua precedência sobre o limite, sem iniciar o aplicativo."""
import ast,json,os,subprocess,sys,tempfile
from pathlib import Path
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve()
commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp,CP_AUTH_TOKEN='owner');os.chdir(tmp);sys.path.insert(0,str(reference/'backend'))
 from fastapi import FastAPI
 from fastapi.testclient import TestClient
 from starlette.middleware.cors import CORSMiddleware
 from starlette.responses import Response
 from app import peers_api
 source=ast.parse((reference/'backend/app/api.py').read_text())
 nodes=[n for n in source.body if isinstance(n,ast.ClassDef) and n.name in ['_BodySizeLimitMiddleware','_BodyTooLarge']]
 namespace={};exec(compile(ast.Module(body=nodes,type_ignores=[]),'original-body-limit','exec'),namespace)
 def make(cors):
  app=FastAPI();app.include_router(peers_api.peers_router)
  @app.get('/synthetic-error')
  def error():return Response('blocked',status_code=403,headers={'Vary':'Accept-Encoding'})
  app.add_middleware(namespace['_BodySizeLimitMiddleware'],max_bytes=32)
  if cors:app.add_middleware(CORSMiddleware,allow_origins=['*'],allow_methods=['*'],allow_headers=['*'],allow_credentials=False,expose_headers=['ETag','x-hangar-css-width','x-hangar-css-height'])
  return TestClient(app,follow_redirects=False)
 plain=make(False);cors=make(True);cases=[]
 def capture(method,path,headers):
  def result(client):
   r=client.request(method,path,headers=headers)
   return dict(status=r.status_code,headers=dict(r.headers),body=r.text)
  cases.append(dict(method=method,path=path,headers=headers,base=result(plain),expected=result(cors)))
 for method in ['DELETE','GET','HEAD','OPTIONS','PATCH','POST','PUT','TRACE','get']:
  for extra in [{},{'access-control-request-headers':'X-Custom, authorization'},{'cookie':'session=fake','content-length':'999'}]:
   capture('OPTIONS','/absent',{'origin':'https://client.invalid','access-control-request-method':method,**extra})
 for method,path in [('GET','/api/peers/ping'),('HEAD','/api/peers/ping'),('GET','/absent'),('GET','/synthetic-error')]:
  for extra in [{},{'cookie':'session=fake'},{'cookie':''},{'content-length':'999'}]:capture(method,path,{'origin':'https://client.invalid',**extra})
 for headers in [{},{'access-control-request-method':'GET'},{'origin':'null'},{'origin':''},{'origin':'https://client.invalid','access-control-request-method':'GET','access-control-request-headers':''}]:capture('OPTIONS','/api/peers/ping',headers)
 output.write_text(json.dumps(dict(source_commit=commit,command='python generate-cors.py <checkout-Python> <fixture>',cases=cases),ensure_ascii=False,indent=2)+'\n')
 print(f'{len(cases)} contratos CORS capturados')
