"""Congela respostas da FastAPI original sem iniciar o ciclo de vida do servidor.

HANGAR_ORQ_REFERENCE_ROOT aponta para o checkout do source_commit quando o Python já saiu.
"""
import sys,json,os,subprocess
from pathlib import Path
root=Path(os.environ.get('HANGAR_ORQ_REFERENCE_ROOT') or Path(__file__).resolve().parents[5]);sys.path.insert(0,str(root/'backend'))
from fastapi.testclient import TestClient
from app import api
api.settings.auth_token='fixture-owner'
api.app.dependency_overrides[api._transfer_guard]=lambda:None
api.app.dependency_overrides[api._transfer_check]=lambda:None
client=TestClient(api.app,raise_server_exceptions=False)
valid={'papel':'executor','provider':'claude','conta':'padrao','mtime':0}
cases=[]
requests=[]
for method,path in [('PUT','/api/orquestracao/politica/account'),('POST','/api/sessions/planner/orq/papel'),('POST','/api/sessions/planner/orq/papeis'),('DELETE','/api/sessions/planner/orq/papel'),('POST','/api/sessions/planner/orq/grupo')]:
    for value in [{},[],None,{'unknown':True}]:
        requests.append((method,path,value))
for key,value in [('papel',42),('provider',False),('headless','what'),('mtime','bad'),('mtime',None),('jev',[]),('vez',None),('unknown',True)]:
    requests.append(('POST','/api/sessions/planner/orq/papel',valid|{key:value}))
requests += [('POST','/api/sessions/planner/orq/papeis',{'papeis':[valid|{'unknown':True}],'mtime':0}),('POST','/api/sessions/planner/orq/papeis',{'papeis':'bad','mtime':0}),('POST','/api/sessions/planner/orq/comecar',{'unknown':True})]
for method,path,value in requests:
    content=json.dumps(value,ensure_ascii=False)
    response=client.request(method,path,headers={'authorization':'Bearer fixture-owner','content-type':'application/json'},content=content)
    try: output=response.json()
    except: output=response.text
    cases.append(dict(method=method,path=path,body=content,content_type='application/json',status=response.status_code,output=output))
for content,content_type in [('','application/json'),('{','application/json'),('{"x":','application/json'),('[}','application/json'),('{} {}','application/json'),('texto','text/plain')]:
    response=client.post('/api/sessions/planner/orq/papel',headers={'authorization':'Bearer fixture-owner','content-type':content_type},content=content)
    cases.append(dict(method='POST',path='/api/sessions/planner/orq/papel',body=content,content_type=content_type,status=response.status_code,output=response.json()))
print(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'command':'python crates/hangar-server/src/orchestration/fixtures/generate_http.py','cases':cases},ensure_ascii=False,indent=2))
