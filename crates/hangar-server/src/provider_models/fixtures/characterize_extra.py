import asyncio,sys,json,tempfile,types,hashlib
from pathlib import Path
from unittest.mock import patch,AsyncMock,Mock
from fastapi import FastAPI,WebSocketDisconnect
from fastapi.testclient import TestClient
reference=Path(sys.argv[1]); output=Path(sys.argv[2]);sys.path.insert(0,str(reference/'backend'))
from app import api
REV='d2a42410f9fe6df4ea491b296c7fea7b98d8b844'
result={'source_commit':REV,'generator':'Python characterize_extra.py <referência congelada> <fixtures>', 'source_files_sha256':{n:hashlib.sha256((reference/'backend/app'/n).read_bytes()).hexdigest() for n in ['api.py','pi_models.py','kimi_models.py','pi_inbox.py','pi_catalog.py']}}
bodyapp=FastAPI()
@bodyapp.post('/pi')
def pi(body:api.PiModelBody): return body.model_dump()
@bodyapp.post('/kimi')
def kimi(body:api.KimiModelBody): return body.model_dump()
c=TestClient(bodyapp);result['bodies']=[]
for provider in ['pi','kimi']:
 for content_type in [None,'application/json','text/plain','application/vnd.fixture+json','application/json; charset=utf-8']:
  for body in ['{}','null','[]','{"model":2}','{']:
   kw={} if content_type is None else {'headers':{'content-type':content_type}}
   response=c.post('/'+provider,content=body,**kw)
   result['bodies'].append({'provider':provider,'content_type':content_type,'body':body,'status':response.status_code,'expected':response.json()})
# A rota original recebe o socket real do TestClient, sem executar o lifespan.
api.settings.auth_token='fixture-owner'; api._WS_PING=0.01
result['sockets']=[]
for name,initial,next_message in [('missing',{'none':1},None),('first-large','x'*(256*1024+10),None),('later-large',{'pane':'fixture-large'},'x'*(256*1024+10)),('heartbeat',{'pane':'fixture-heartbeat'},None)]:
 c=TestClient(api.app,client=('127.0.0.1',1234));pings=0
 try:
  with c.websocket_connect('/api/pi/inbox?token=fixture-owner') as ws:
   if isinstance(initial,str):ws.send_text(initial)
   else:ws.send_json(initial)
   if next_message:ws.send_text(next_message)
   while True:
    value=ws.receive_json()
    if value.get('ping'):pings+=1
 except WebSocketDisconnect as error:result['sockets'].append({'name':name,'close_code':error.code,'pings':pings})
result['routes']=[]
pi_cat={'current':{'provider':'p','id':'one','name':'One'},'thinking':'low','levels':['low','high'],'models':[{'provider':'p','id':'one','name':'One','reasoning':True}]}
kimi_cat={'default':'p/one','models':[{'alias':'p/one','provider':'p','id':'one','name':'One','context_length':128000,'efforts':['low','high'],'default_effort':'high'}]}
for provider in ['pi','omp','kimi','claude']:
 for source in ['present','missing']:
  info=types.SimpleNamespace(provider=provider,jsonl='/fixture/life.jsonl')
  for endpoint in ['pi','kimi']:
   patches=[patch.object(api,'_cached_info',AsyncMock(return_value=info)),patch.object(api,'_session_config_dir',return_value=None),patch.object(api.pi_models,'read_catalog',return_value=pi_cat if source=='present' else None),patch.object(api.kimi_models,'read_catalog',return_value=kimi_cat if source=='present' else None)]
   for p in patches:p.start()
   try:
    try:status,value=200,asyncio.run((api.pi_models_list if endpoint=='pi' else api.kimi_models_list)('fixture'))
    except api.HTTPException as error:status,value=error.status_code,{'detail':error.detail}
    result['routes'].append({'provider':provider,'source':source,'endpoint':endpoint,'catalog':pi_cat if endpoint=='pi' else kimi_cat,'status':status,'expected':value})
   finally:
    for p in reversed(patches):p.stop()
data=result;pc=pi_cat;kc=kimi_cat;data["validation"]=[]
for op,inputs in [('check_pi',[[pc,'p','one'],[pc,'p','missing'],[pc,'other','one']]),('check_kimi',[[kc,'p/one'],[kc,'p/missing'],[kc,'bad alias']]),('check_effort',[[kc['models'][0],'LOW'],[kc['models'][0],'xhigh'],[{'efforts':[]},'low']])]:
 for args in inputs:
  try:
   value=({'check_pi':api.pi_models.check_known,'check_kimi':api.kimi_models.check_known,'check_effort':api.kimi_models.check_effort}[op])(*args)
   expected={'ok':value}
  except (api.pi_models.PiModelError,api.kimi_models.KimiModelError) as error:expected={'error':{'status':error.status,'detail':error.detail}}
  data['validation'].append({'op':op,'input':args,'expected':expected})
data['posts']=[]
for provider,body in [('pi',{}),('pi',{'model':'one'}),('pi',{'provider':'p','model':'missing'}),('pi',{'effort':'wrong'}),('pi',{'provider':'p','model':'one','effort':'high'}),('kimi',{}),('kimi',{'model':'p/missing'}),('kimi',{'model':'p/one','effort':'xhigh'}),('kimi',{'model':'p/one','effort':'low'})]:
 cat=pc if provider=='pi' else kc;driver=Mock(return_value={'ok':True,'result':'Switched to One with thinking low for this session only.'});after={**pc,'thinking':'high'}
 with patch.object(api,'_pi_catalog',AsyncMock(return_value=(pc,'/fixture/life.jsonl'))),patch.object(api,'_kimi_info',AsyncMock(return_value=type('Info',(),{'jsonl':None})())),patch.object(api,'_kimi_catalog',return_value=kc),patch.object(api,'_session_config_dir',return_value=None),patch.object(api,'_recusa_se_painel_aberto'),patch.object(api.terminal,'send_pi_commands',driver),patch.object(api.terminal,'set_kimi_model',driver),patch.object(api.pi_models,'read_back',return_value=after):
  try:status,value=200,asyncio.run((api.pi_model_set if provider=='pi' else api.kimi_model_set)('fixture',(api.PiModelBody if provider=='pi' else api.KimiModelBody)(**body)))
  except api.HTTPException as error:status,value=error.status_code,{'detail':error.detail}
  data['posts'].append({'provider':provider,'catalog':cat,'after':after,'body':body,'status':status,'expected':value,'effects':[list(c.args) for c in driver.call_args_list]})
result['globals']=[]
for provider in ['pi','omp','kimi']:
 for source in ['present','missing','failure'] if provider!='kimi' else ['present','missing']:
  failure=api.pi_catalog.PiAusente('fixture absent') if source=='missing' else RuntimeError('fixture failure')
  cat=pc['models'] if provider!='kimi' else kc
  with patch.object(api.pi_catalog,'listar',side_effect=failure if source!='present' else None,return_value=cat),patch.object(api.kimi_models,'read_catalog',return_value=kc if source=='present' else None):
   try:status,value=200,asyncio.run(api.model_options_sem_sessao(provider=provider))
   except api.HTTPException as error:status,value=error.status_code,{'detail':error.detail}
  result['globals'].append({'provider':provider,'source':source,'catalog':cat,'message':str(failure),'status':status,'expected':value})
import uuid
async def capture_generated_ids():
 from app.pi_inbox import PiInbox
 inbox=PiInbox();frames=[]
 async def outgoing(value):
  frames.append(value)
  if 'pedir' in value:inbox.responder('%1',value['id'],'')
  else:inbox.confirmar('%1',value['id'],True,None)
 inbox.registrar('%1',outgoing)
 seed='0'*32
 with patch('app.pi_inbox.uuid.uuid4',return_value=uuid.UUID(hex=seed,version=4)):
  assert await inbox.entregar('%1','fixture')=='sent'
  assert await inbox.perguntar('%1','editor')==''
 return {'seed':seed,'frames':frames}
result['generated_ids']=asyncio.run(capture_generated_ids())
output.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print(len(result['bodies']),result['sockets'],len(result['routes']))
