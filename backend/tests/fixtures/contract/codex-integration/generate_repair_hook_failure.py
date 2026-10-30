"""Mede o falso preparo original após o instalador absorver falhas de hooks."""
import ast, asyncio, hashlib, json, os, shlex, subprocess, sys, tempfile
from contextlib import asynccontextmanager, nullcontext
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
def extract(path,names,ns):
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[n for n in ast.parse(source).body if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef,ast.ClassDef)) and n.name in names]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),ns)
cases=[]
for name,raw in [('broken',b'{'),('array',b'[]'),('root-null',b'null'),('hooks-null',b'{"hooks":null}'),('hooks-array',b'{"hooks":[]}'),('utf8',b'\xff'),('bom',b'\xef\xbb\xbf{}'),('python-space',b'\x1c'),('empty',b''),('valid',b'{}')]:
 with tempfile.TemporaryDirectory() as directory:
  home=Path(directory);account=SimpleNamespace(id='default',home=home/'.codex',is_default=True);account.home.mkdir();path=account.home/'hooks.json';path.write_bytes(raw);calls=[]
  scripts=home/'repo/backend/hooks';scripts.mkdir(parents=True);python=home/'repo/python/bin/python3';python.parent.mkdir(parents=True);python.touch()
  @asynccontextmanager
  async def exclusive(path):yield
  async def complete(work):return await work
  class Native:
   def __init__(self,*args,**kwargs):pass
   async def __aenter__(self):calls.append('open');return self
   async def __aexit__(self,*args):calls.append('close')
   async def request(self,method,params):calls.append(method);return {'data':[]}
  class Log:
   def exception(self,*args,**kwargs):calls.append(type(sys.exc_info()[1]).__name__)
  state=str(scripts/'state_hook.py');nav=str(scripts/'nav_hook.py');guard=str(scripts/'guard_tmux.py')
  ns=dict(asyncio=asyncio,Path=Path,Account=SimpleNamespace,json=json,os=os,tempfile=tempfile,hashlib=hashlib,shlex=shlex,
   atomico=SimpleNamespace(substituir=os.replace),account_lifecycle=SimpleNamespace(AccountKey=SimpleNamespace(new=lambda *args:args),acquire=lambda key:nullcontext(),complete_on_cancel=complete),
   exclusivo=exclusive,resolve_account=lambda id:account,PREPARED_MARKER='.hangar-prepared-v1',CodexNativo=Native,shutil=SimpleNamespace(which=lambda name:'/fixture/codex'),
   adaptar_hooks_plugin=lambda *args,**kwargs:None,STATE_HOOK=state,NAV_HOOK=nav,GUARD_HOOK=guard,_PYTHON=str(python),_log=Log(),
   _EVENTOS=['SessionStart','UserPromptSubmit','PreToolUse','PostToolUse','Stop'],_STATE_COMMAND=f'"{python}" "{state}" || exit 0',_NAV_COMMAND=f'"{python}" "{nav}" || exit 0')
  extract('backend/app/codex_arquivos.py',{'ler','json_bytes','gravar','backup','AlteradoExternamente'},ns)
  extract('backend/app/hook_installer.py',{'_tokens','_refers_to','_load_settings','_write'},ns)
  extract('backend/app/codex_hook_installer.py',{'comando_estado','comando_navegador','comando_guarda','_tem_hook','_reescrever_formato_cmd','_reparar_python_legado','ensure_codex_state_hook_installed'},ns)
  extract('backend/app/codex_compat.py',{'edicoes_confianca_hooks'},ns)
  extract('backend/app/codex_preparo.py',{'reparar_conta','_reparar_conta_locked'},ns)
  package=ModuleType('app');package.__path__=[];sync=ModuleType('app.codex_contas_sync');sync.preparation_status=lambda _: {'status':'ready'}
  integration=ModuleType('app.codex_integracao');integration.IntegracaoCodex=lambda **kwargs:SimpleNamespace(status=lambda:{'estado':'ok'})
  with patch.object(Path,'home',return_value=home),patch.dict(sys.modules,{'app':package,'app.codex_contas_sync':sync,'app.codex_integracao':integration}):
   try:result={'ok':asyncio.run(ns['reparar_conta'](account,adotar_preparo=True))}
   except Exception as error:result={'error':type(error).__name__,'message':str(error)}
  final=path.read_bytes().replace(directory.encode(),b'$ROOT')
  cases.append(dict(name=name,input_hex=raw.hex(),result=result,calls=calls,final_hex=final.hex(),marker=(account.home/'.hangar-prepared-v1').read_text() if (account.home/'.hangar-prepared-v1').exists() else None))
Path(__file__).with_name('repair-hook-failure.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_repair_hook_failure.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
for case in cases:print(case['name'],case['result'],case['calls'],bool(case['marker']))
