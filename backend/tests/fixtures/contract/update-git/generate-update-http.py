"""Captura roteamento e consulta do updater, mantendo decorators e assinatura originais."""
import argparse,ast,asyncio,json,subprocess
from pathlib import Path
from fastapi import FastAPI,Depends
import httpx
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args();reference=args.reference.resolve();output=args.output.resolve()
app=FastAPI();tree=ast.parse((reference/'backend/app/api.py').read_text());calls=[]
def require_auth():calls.append('auth')
namespace={'app':app,'Depends':Depends,'require_auth':require_auth}
for node in tree.body:
 if isinstance(node,ast.AsyncFunctionDef) and node.name in ['get_atualizacao','post_atualizacao_iniciar','post_atualizacao_reiniciar']:
  node.body=ast.parse('return {"called": True, "procurar": procurar}' if node.name=='get_atualizacao' else 'return {"called": True}').body
  exec(compile(ast.fix_missing_locations(ast.Module(body=[node],type_ignores=[])),'<original-route-signature>','exec'),namespace)
async def capture():
 rows=[]
 async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app),base_url='http://fixture') as client:
  cases=[(method,path) for method in ['GET','HEAD','POST','PUT','PATCH','DELETE','OPTIONS'] for path in ['/api/atualizacao','/api/atualizacao/','/api/atualizacao/iniciar','/api/atualizacao/reiniciar','/api/atualizacao/unknown','/api/%61tualizacao/']]
  cases += [('GET','/api/atualizacao?'+query) for query in ['procurar=TRUE','procurar=yes','procurar=on','procurar=0','procurar=False','procurar=no','procurar=off','procurar=','procurar=2','procurar=bad','procurar=1&procurar=0','procurar=0&procurar=1','procurar=%74rue']]
  for method,path in cases:
   calls.clear();response=await client.request(method,path,follow_redirects=False)
   rows.append({'method':method,'path':path,'status':response.status_code,'body':response.text,'headers':{k:v for k,v in response.headers.items() if k in ['content-type','content-length','allow','location']},'calls':list(calls)})
 return rows
rows=asyncio.run(capture());output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip(),'command':'python generate-update-http.py --reference <checkout-Python> --output <fixture>','scope':'Decorators e assinatura originais; corpo substituído para observar despacho sem efeitos.','cases':rows},ensure_ascii=False,indent=2)+'\n');print(len(rows))
