"""Captura a serialização HTTP do estado pelo handler original, sem serviços reais."""
import argparse,ast,asyncio,json,pathlib,subprocess,types
from fastapi import FastAPI,Depends
from fastapi.testclient import TestClient
parser=argparse.ArgumentParser();parser.add_argument('--reference',type=pathlib.Path,required=True);parser.add_argument('--output',type=pathlib.Path,required=True);args=parser.parse_args();root=args.reference.resolve();tree=ast.parse((root/'backend/app/api.py').read_text());node=next(n for n in tree.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='get_atualizacao');app=FastAPI();state={}
async def require_auth():pass
ns={'app':app,'Depends':Depends,'require_auth':require_auth,'asyncio':asyncio,'atualizar':types.SimpleNamespace(checar=lambda:{'branch':'main','alvo':'main'},alvo=lambda:'main',_troca_de_branch=lambda pre:False,estado_para_tela=lambda:state),'_publicado':lambda *args:('origin/main',False,1),'_mudancas_pendentes':lambda *args:[],'diag':types.SimpleNamespace(_git_describe=lambda:'fixture',VERSAO_EM_EXECUCAO='fixture',versao_legivel=lambda *args:'fixture',VERSAO_LEGIVEL_EM_EXECUCAO='fixture'),'atualizacoes':types.SimpleNamespace(pendentes=lambda:[])}
exec(compile(ast.Module(body=[node],type_ignores=[]),'<original-http-state>','exec'),ns)
rows=[]
for name,raw in [('plain','{"fase":"pronto"}'),('nan','{"keep":NaN}'),('infinity','{"keep":Infinity}'),('surrogate','{"keep":"\\ud800"}'),('unicode','{"keep":"é"}'),('deep','{"keep":'+'['*1500+'0'+']'*1500+'}')]:
 state=json.loads(raw)
 with TestClient(app,raise_server_exceptions=False) as client:response=client.get('/api/atualizacao')
 row={'name':name,'input':raw,'status':response.status_code,'body':response.text,'content_type':response.headers.get('content-type')}
 if response.status_code==500:
  try:
   with TestClient(app) as client:client.get('/api/atualizacao')
  except Exception as error:row['exception']=type(error).__name__;row['message']=str(error)
 rows.append(row)
args.output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'command':'python backend/tests/fixtures/contract/update-git/generate-state-http.py --reference <checkout-Python> --output <fixture>','scope':'Handler original por AST; fontes git/versão/passos injetadas; estado real carregado por json.loads.','cases':rows},ensure_ascii=True,indent=2))
print([(r['name'],r['status'],r.get('exception')) for r in rows])
