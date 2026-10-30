"""Executa adoção e validação do reparo original com travas observáveis e disco isolado."""
import ast, asyncio, hashlib, json, os, subprocess, sys, tempfile
from contextlib import contextmanager, asynccontextmanager
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
def extract(path,names,namespace):
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[n for n in ast.parse(source).body if isinstance(n,(ast.FunctionDef,ast.AsyncFunctionDef,ast.ClassDef)) and n.name in names]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),namespace)
cases=[]
for default in [False,True]:
 for ready in [False,True]:
  for adopt in [False,True]:
   for outcome in [None,False,True,'error','changed']:
    with tempfile.TemporaryDirectory() as raw:
     home=Path(raw);account=SimpleNamespace(id='fixture',home=home/'account',is_default=default);account.home.mkdir();calls=[]
     marker=account.home/'.hangar-prepared-v1'
     @contextmanager
     def acquire(key):
      calls.append('lock-enter')
      try:yield
      finally:calls.append('lock-exit')
     @asynccontextmanager
     async def exclusive(path):
      calls.append('exclusive-enter')
      try:yield
      finally:calls.append('exclusive-exit')
     async def complete(work):return await work
     async def repair(value):
      calls.append('repair')
      if outcome=='error':raise OSError('fixture repair')
      return outcome
     def resolve(id):
      calls.append('resolve')
      return SimpleNamespace(home=home/'other' if outcome=='changed' else account.home)
     sync=ModuleType('app.codex_contas_sync')
     def status(value):calls.append('extra-status');return {'status':'ready' if ready else 'partial'}
     sync.preparation_status=status
     integration=ModuleType('app.codex_integracao')
     class Integration:
      def __init__(self,**kwargs):pass
      def status(self):calls.append('default-status');return {'estado':'ok' if ready else 'parcial'}
     integration.IntegracaoCodex=Integration
     package=ModuleType('app');package.__path__=[]
     ns=dict(asyncio=asyncio,Path=Path,Account=SimpleNamespace,json=json,os=os,tempfile=tempfile,hashlib=hashlib,
      atomico=SimpleNamespace(substituir=os.replace),PREPARED_MARKER=marker.name,
      account_lifecycle=SimpleNamespace(AccountKey=SimpleNamespace(new=lambda *args:args),acquire=acquire,complete_on_cancel=complete),
      exclusivo=exclusive,resolve_account=resolve,_reparar_conta_locked=repair)
     extract('backend/app/codex_arquivos.py',{'ler','json_bytes','gravar','backup','AlteradoExternamente'},ns)
     extract('backend/app/codex_preparo.py',{'reparar_conta'},ns)
     with patch.dict(sys.modules,{'app':package,'app.codex_contas_sync':sync,'app.codex_integracao':integration}):
      try:result={'ok':asyncio.run(ns['reparar_conta'](account,adotar_preparo=adopt))}
      except Exception as error:result={'error':type(error).__name__,'message':str(error)}
     cases.append(dict(default=default,ready=ready,adopt=adopt,outcome=outcome,result=result,calls=calls,marker=marker.read_text() if marker.exists() else None))
Path(__file__).with_name('repair-lifecycle.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_repair_lifecycle.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
