"""Captura contratos puros e efeitos locais do painel, sem backend ou terminal real."""
import contextlib,io,json,os,runpy,subprocess,sys,tempfile
from pathlib import Path
from unittest.mock import patch
reference=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]).resolve();commit=subprocess.check_output(['git','-C',str(reference),'rev-parse','HEAD'],text=True).strip()
rows=[]
with tempfile.TemporaryDirectory() as tmp:
 os.environ.update(HOME=tmp,USERPROFILE=tmp);os.chdir(tmp)
 data=runpy.run_path(str(reference/'scripts/hangar-panel-data'));action=runpy.run_path(str(reference/'scripts/hangar-panel-action'))
 common=sys.modules['hangar_panel_common']
 for cfg in [None,False,0,'bad',[],{}, {'enabled':False},{'enabled':0},{'enabled':'false'}, {'base_url':' http://peer/ ','token':' token ','web_url':' https://web/'},{'base_url':12,'token':False}]:rows.append(dict(kind='peer_fields',input=cfg,fields=common.peer_fields(cfg),enabled=common.enabled(cfg)))
 for session in [{'name':'one'},{'name':'two','cwd':'relative/missing','state':'working','extra':'discarded'},{'name':'three','cwd':'','state':'awaiting_input','pair_peers':['p']}]:
  for server,web in [(None,''),('remote','https://peer/')]:rows.append(dict(kind='slim',input=session,server=server,web=web,result=data['slim'](session,server,web,{'one'})))
 for cfg in [None,[],{'scripts':[]},{'scripts':{'start':'x'}},{'scripts':{'dev':'x','start':'y'}},{'scripts':{'dev':None},'packageManager':'pnpm@9'},{'scripts':{'dev':'x'},'packageManager':'unknown@1'}]:
  for locks in [[],['pnpm-lock.yaml','yarn.lock'],['yarn.lock'],['bun.lockb'],['package-lock.json']]:
   path=Path(tmp)/'project';path.mkdir(exist_ok=True)
   for p in path.iterdir():p.unlink()
   (path/'package.json').write_text(json.dumps(cfg))
   for name in locks:(path/name).write_text('')
   rows.append(dict(kind='detect_command',package=cfg,locks=locks,result=action['_detect_command'](str(path))))
 for args in [[],['peer','peer'],['peer','peer','bad'],['session','rename'],['session','rename','  '],['session','unknown']]:
  out=io.StringIO()
  with patch.object(sys,'argv',['hangar-panel-action',*args]),contextlib.redirect_stdout(out):rc=action['main']()
  rows.append(dict(kind='dispatch',args=args,rc=rc,result=json.loads(out.getvalue())))
 for rows_in in ['1\tobserver\n0\thuman\n0\thuman\n','\tunknown\n0\t\n','bad\n']:
  with patch.object(subprocess,'run',return_value=subprocess.CompletedProcess([],0,rows_in,'')):result=sorted(data['attached_locally']())
  rows.append(dict(kind='attached',input=rows_in,result=result))
 for known in [{},{'disabled':{'enabled':False},'bad':'invalid','app':{'app':{}},'remote':{'base_url':'http://remote','token':'t'}},{'local':{'base_url':'http://bad','token':'t'}}]:
  def fetch(base,token):
   if base=='http://bad':raise common.PanelError('falha sintética')
   return [{'name':'z','state':'idle'},{'name':'a','state':'working'},{'name':'b','state':'awaiting_input'}]
  globals_data=data['main'].__globals__
  overrides={'env':lambda key:'token','peers':lambda:known,'local_base':lambda:'http://local','attached_locally':lambda:{'a'},'fetch':fetch,'fetch_projects':lambda base,token:([{'name':'project','cwd':'relative'}],'')}
  out=io.StringIO()
  with patch.dict(globals_data,overrides),contextlib.redirect_stdout(out):rc=data['main']()
  rows.append(dict(kind='snapshot',peers=known,rc=rc,result=json.loads(out.getvalue())))
 specs=[
  (['session','rename','new'],[{'name':'new'}]),(['session','kill'],[{}]),
  (['session','git'],[{'ok':True,'output':'## main\n M file\n?? extra\n'}]),
  (['session','git','pull'],[{'ok':False,'output':'fatal: failed\nexplanation\n'}]),
  (['session','git','fetch'],[{'ok':True,'output':'updated\nsecond\n'}]),
  (['session','push'],[{}]),(['session','commit'],[[]]),
  (['session','commit','text'],[[{'path':'one'},'two'],{}]),
  (['session','commit-push','text'],[[{'path':'one'}],{},{}]),
  (['session','commit-push','text'],[[{'path':'one'}],{},common.PanelError('503: refused')]),
  (['session','info'],[[{'name':'session','branch':'main'}],{'files':['one']}]),
  (['session','info'],[[{'name':'session'}],common.PanelError('not a git repository')]),
  (['session','info'],[[{'name':'session'}],common.PanelError('401: refused')]),
  (['project','project','start'],[{'state':'working'}]),(['project','project','stop'],[{}]),
  (['project','project','pane'],[{'pane':'output'}]),(['project','project','bad'],[]),
  (['project','project-add','cwd','command','123'],[{'name':'project'}]),
  (['project','project-add','cwd','command','bad'],[]),(['project','project-del'],[{}]),
 ]
 for args,responses in specs:
  calls=[];pending=list(responses)
  def api(*args,**kwargs):
   calls.append(dict(args=args,kwargs=kwargs));result=pending.pop(0)
   if isinstance(result,Exception):raise result
   return result
  out=io.StringIO()
  with patch.dict(action['main'].__globals__,{'api':api}),patch.object(sys,'argv',['hangar-panel-action',*args]),contextlib.redirect_stdout(out):rc=action['main']()
  rows.append(dict(kind='action_api',args=args,responses=[{'error':str(x)} if isinstance(x,Exception) else x for x in responses],calls=calls,rc=rc,result=json.loads(out.getvalue())))
 import urllib.error
 for kind,status,body in [('ok',200,''),('ok',200,'{"ok":true}'),('ok',200,'plain'),('http',404,'{"detail":"sessão ausente"}'),('http',503,'broken'),('network',0,'connection refused')]:
  calls=[]
  def urlopen(request,timeout):
   calls.append(dict(url=request.full_url,method=request.method,body=request.data.decode() if request.data else None,headers=dict(request.header_items()),timeout=timeout))
   if kind=='http':raise urllib.error.HTTPError(request.full_url,status,'Synthetic reason',{},io.BytesIO(body.encode()))
   if kind=='network':raise urllib.error.URLError(body)
   return io.BytesIO(body.encode())
  with patch.object(common,'resolve',return_value=('https://peer.invalid','fixture-token','name/a b')),patch.object(common.urllib.request,'urlopen',urlopen):
   try:result={'value':common.api('remote::name/a b','POST','/api/sessions/{name}/test',{'message':'ação'},timeout=30)}
   except common.PanelError as error:result={'error':str(error)}
  rows.append(dict(kind='transport',response_kind=kind,status=status,body=body,calls=calls,result=result))
 text=json.dumps(dict(source_commit=commit,command='python generate-panel.py <checkout-Python> <fixture>',cases=rows),ensure_ascii=False,indent=2).replace(tmp,'<ROOT>')
 output.write_text(text+'\n');print(f'{len(rows)} contratos do painel capturados')
