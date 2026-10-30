"""Captura seleção de contas e recarga de sessões do reparo pontual."""
import ast, asyncio, hashlib, ipaddress, json, os, runpy, subprocess, sys, tempfile
from pathlib import Path
from types import SimpleNamespace, ModuleType
from unittest.mock import patch
from urllib.parse import urlparse
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
ns=dict(asyncio=asyncio,Path=Path,ipaddress=ipaddress,urlparse=urlparse,runpy=runpy,
 json=json,os=os,hashlib=hashlib,tempfile=tempfile,atomico=SimpleNamespace(substituir=os.replace))
def extract(path,names):
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[n for n in ast.parse(source).body if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef,ast.ClassDef)) and n.name in names]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),ns)
extract('backend/app/codex_arquivos.py',{'ler','json_bytes','gravar','backup','AlteradoExternamente'})
extract('backend/app/codex_preparo.py',{'reparar_todas'})
async def complete(task): return await task
ns['account_lifecycle']=SimpleNamespace(complete_on_cancel=complete)
cases=[]
for name in ['empty','no-sessions','valid','ipv6','localhost','remote','invalid-host','invalid-url',
             'dead','missing-pid','bool-pid','unknown-account','missing-endpoint','missing-home',
             'no-cwd','empty-cwd','reload-false','repair-failure','missing-account-dir','two-sessions']:
 with tempfile.TemporaryDirectory() as raw:
  root=Path(raw);account=root/'account';account.mkdir();calls=[]
  accounts=[] if name=='empty' else [SimpleNamespace(id='fixture',home=account)]
  if name=='missing-account-dir':accounts.append(SimpleNamespace(id='missing',home=root/'missing'))
  session=dict(endpoint='ws://127.0.0.1:1234',codex_home=str(account),app_pid=42,cwd='/fixture/project')
  if name=='ipv6':session['endpoint']='ws://[::1]:1234'
  if name=='localhost':session['endpoint']='ws://localhost:1234'
  if name=='remote':session['endpoint']='ws://192.0.2.1:1234'
  if name=='invalid-host':session['endpoint']='ws://fixture.invalid:1234'
  if name=='invalid-url':session['endpoint']='not-a-url'
  if name=='missing-pid':session.pop('app_pid')
  if name=='bool-pid':session['app_pid']=True
  if name=='unknown-account':session['codex_home']=str(root/'other')
  if name=='missing-endpoint':session.pop('endpoint')
  if name=='missing-home':session.pop('codex_home')
  if name=='no-cwd':session.pop('cwd')
  if name=='empty-cwd':session['cwd']=''
  sessions=[] if name in ('empty','no-sessions','missing-account-dir') else [session]*(2 if name=='two-sessions' else 1)
  async def repair(value,**kwargs):
   calls.append(['repair',value.id,kwargs])
   if name=='repair-failure':raise RuntimeError('fixture repair')
  def reload(endpoint,cwd,**kwargs):calls.append(['reload',endpoint,cwd,{k:str(v) if isinstance(v,Path) else v for k,v in kwargs.items()}]);return name!='reload-false'
  def list_sessions(**kwargs):calls.append(['sessions',kwargs]);return sessions
  mod=ModuleType('app.adapters.codex');mod.sessions=SimpleNamespace(list_all=list_sessions)
  proc=ModuleType('app.procinfo');proc.pid_vivo=lambda pid:name!='dead'
  ns.update(list_accounts=lambda:accounts,reparar_conta=repair,install_root=SimpleNamespace(root=lambda:root/'repo'))
  with patch.dict(sys.modules,{'app.adapters.codex':mod,'app.procinfo':proc}),patch.object(Path,'home',return_value=root),patch.object(runpy,'run_path',return_value={'_confiar_hooks':reload}):
   try:asyncio.run(ns['reparar_todas']());result={'ok':None}
   except Exception as error:result={'error':type(error).__name__,'message':str(error)}
  proof=root/'.hangar/codex-prepare-once-v1.json'
  cases.append(json.loads(json.dumps(dict(name=name,sessions=sessions,result=result,calls=calls,proof=proof.read_text() if proof.exists() else None)).replace(raw,'$ROOT')))
Path(__file__).with_name('repair-all.json').write_text(json.dumps(dict(source_commit=COMMIT,
 command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_repair_all.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
