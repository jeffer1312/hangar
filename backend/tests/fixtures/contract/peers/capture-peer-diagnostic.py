import os,sys,tempfile,subprocess,json,io,urllib.error,types
from pathlib import Path
from unittest.mock import MagicMock
ref=Path(sys.argv[1]).resolve(); out=Path(sys.argv[2]); commit=subprocess.check_output(['git','-C',str(ref),'rev-parse','HEAD'],text=True).strip()
with tempfile.TemporaryDirectory(prefix='peer-diagnostic-') as td:
 os.environ.update(HOME=td,USERPROFILE=td);os.chdir(td);sys.path.insert(0,str(ref/'backend'))
 from app import diag
 module=types.ModuleType('original_peers');module.__file__=str(Path(td)/'peers.py');exec(compile(subprocess.check_output(['git','-C',str(ref),'show',f'{commit}:backend/app/peers.py']),module.__file__,'exec'),module.__dict__)
 original_record=diag.registrar
 rows=[];secret='segredo-que-nao-pode-aparecer@example.invalid'
 for result in ['ok','http','transporte','json']:
  events=[];headers=[];module.peer_cfg=lambda _:('http://peer.invalid',secret)
  def record(event,level='ok',**fields):events.append(dict(evento=event,nivel=level,req=diag.req_atual.get(),operacao=diag._operacao_atual.get(),**fields))
  diag._base=lambda:Path(td)/result
  diag.registrar=original_record
  def opening(request,**kwargs):
   headers.append(request.get_header('X-hangar-req'))
   if result=='http':raise urllib.error.HTTPError(request.full_url,403,secret,{},io.BytesIO(secret.encode()))
   if result=='transporte':raise urllib.error.URLError(TimeoutError(secret))
   response=MagicMock();body=io.BytesIO((secret if result=='json' else json.dumps({'token':secret})).encode());response.__enter__.return_value=types.SimpleNamespace(status=200,read=body.read,read1=body.read1);return response
  module.urllib.request.urlopen=opening;token=diag.req_atual.set('req-diagnostico')
  try: module.call('outro','POST','/api/pair',{'texto':secret});error=None
  except module.PeerError as exc:error={'transport':exc.transport}
  finally:diag.req_atual.reset(token)
  events=[json.loads(line) for line in diag.caminho_do_dia().read_text().splitlines()]
  assert secret not in json.dumps(events)
  for event in events:
   for key in ['ts','backend','pid_backend','origem']:event.pop(key,None)
   if 'ms' in event:assert event['ms']>=0;event['ms']='<MS>'
   assert event['operacao'];event['operacao']='<OPERATION>'
  rows.append(dict(name=result,request=headers,error=error,events=events))
 out.write_text(json.dumps(dict(source_commit=commit,command='python capture-peer-diagnostic.py <reference> <output>',cases=rows),ensure_ascii=False,indent=2)+'\n')
 print(len(rows),'casos capturados')
