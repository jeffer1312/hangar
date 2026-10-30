"""Captura a sequência do reparo original com adaptadores controlados e disco temporário."""
import ast, asyncio, hashlib, json, os, shutil, subprocess, tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
def extract(path,names,namespace):
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[n for n in ast.parse(source).body if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef,ast.ClassDef)) and n.name in names]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),namespace)
async def complete(awaitable):return await awaitable
ns=dict(asyncio=asyncio,Path=Path,Account=SimpleNamespace,json=json,os=os,tempfile=tempfile,hashlib=hashlib,shutil=shutil,
 atomico=SimpleNamespace(substituir=os.replace),account_lifecycle=SimpleNamespace(complete_on_cancel=complete))
extract('backend/app/codex_arquivos.py',{'ler','json_bytes','gravar','backup','AlteradoExternamente'},ns)
extract('backend/app/codex_compat.py',{'edicoes_confianca_hooks'},ns)
extract('backend/app/codex_preparo.py',{'_reparar_conta_locked'},ns)
cases=[]
def hook(source):return dict(enabled=True,trustStatus='untrusted',source=source,key='fixture:hook',currentHash='hash')
for name,data,old,backup,plugins in [
 ('empty',{'data':[]},None,None,[]),('existing',{'data':[]},b'original',None,[]),
 ('backup-kept',{'data':[]},b'current',b'first',[]),('no-cli',{'data':[]},b'original',None,['b','a']),
 ('plugins',{'data':[]},None,None,['b','a']),('user',{'data':[{'hooks':[hook('user')]}]},None,None,[]),
 ('project',{'data':[{'hooks':[hook('project')]}]},None,None,[]),
 ('mixed',{'data':[{'hooks':[hook('user'),hook('project')]}]},None,None,[]),
 ('warnings',{'data':[{'warnings':['warning']}]},b'original',None,[]),
 ('invalid',None,b'original',None,[]),('batch-failure',{'data':[{'hooks':[hook('user')]}]},b'original',None,[]),
 ('plugin-failure',{'data':[]},b'original',None,['b','a']),
]:
 with tempfile.TemporaryDirectory() as raw:
  home=Path(raw);account=SimpleNamespace(id='fixture',home=home/'account',is_default=False);account.home.mkdir();calls=[]
  saved=home/'.hangar/codex-prepare-once/backups/fixture/hooks.json'
  if old is not None:(account.home/'hooks.json').write_bytes(old)
  if backup is not None:saved.parent.mkdir(parents=True);saved.write_bytes(backup)
  for version in plugins:(account.home/'plugins/cache/ecc/ecc'/version).mkdir(parents=True)
  def install(path):calls.append(['hooks',str(path)]);(path/'hooks.json').write_bytes(b'installed')
  def adapt(root,*args,**kwargs):
   calls.append(['plugin',str(root)])
   if name=='plugin-failure':raise ValueError('fixture plugin')
  class Native:
   def __init__(self,*args,**kwargs):pass
   async def __aenter__(self):calls.append(['open']);return self
   async def __aexit__(self,*args):calls.append(['close'])
   async def request(self,method,params):
    calls.append([method,params])
    if method=='config/batchWrite' and name=='batch-failure':raise OSError('fixture batch')
    return data if method=='hooks/list' else {}
  ns.update(ensure_codex_state_hook_installed=install,adaptar_hooks_plugin=adapt,CodexNativo=Native)
  with patch.object(Path,'home',return_value=home),patch.object(shutil,'which',return_value=None if name=='no-cli' else '/fixture/codex'):
   try:result=dict(ok=asyncio.run(ns['_reparar_conta_locked'](account)))
   except Exception as error:result=dict(error=type(error).__name__,message=str(error))
  case=dict(name=name,data=data,original_hex=old.hex() if old else None,backup_hex=backup.hex() if backup else None,plugins=plugins,
   result=result,calls=calls,final_hooks_hex=(account.home/'hooks.json').read_bytes().hex(),
   final_backup_hex=saved.read_bytes().hex() if saved.exists() else None,
   private_mode=(home/'.hangar/codex-prepare-once').stat().st_mode&0o777)
  cases.append(json.loads(json.dumps(case).replace(raw,'$ROOT')))
Path(__file__).with_name('repair.json').write_text(json.dumps(dict(source_commit=COMMIT,
 command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_repair.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
