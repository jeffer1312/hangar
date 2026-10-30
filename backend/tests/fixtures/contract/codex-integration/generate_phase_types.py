"""Captura checkpoints alterados externamente antes de reutilizar a fase."""
import ast,asyncio,json,subprocess
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True)
cls=next(n for n in ast.parse(source).body if isinstance(n,ast.ClassDef) and n.name=='IntegracaoCodex')
method=next(n for n in cls.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_run_phase')
ns=dict(asyncio=asyncio,time=SimpleNamespace(time=lambda:1000.0))
exec(compile(ast.Module(body=[method],type_ignores=[]),'backend/app/codex_integracao.py','exec'),ns)
cases=[]
for field in ['errors','warnings','confirmed','plugins','next_update']:
 for value in [None,False,7,'ab',{'key':1},[]]:
  state=dict(erros=['existing error'],avisos=['existing warning'],plugins=[],proxima_atualizacao=None)
  registry={'phases':{'plugins':dict(fingerprint='same',errors=['old error'],warnings=['old warning'],confirmed=['z','a'],plugins=['old plugin'],next_update='old date')}}
  registry['phases']['plugins'][field]=value
  initial=json.loads(json.dumps(registry));calls=[]
  def fingerprint(**kwargs):calls.append(['fingerprint',kwargs.get('fontes',False)]);return {'plugins':'same'}
  obj=SimpleNamespace(_estado=state,_plugins_confirmados=set(),_phase_fingerprints=fingerprint)
  async def action():raise AssertionError('não deve executar')
  try:asyncio.run(ns['_run_phase'](obj,'plugins',registry,action));result={'ok':None}
  except Exception as error:result={'error':type(error).__name__,'message':str(error)}
  cases.append(dict(field=field,value=value,initial=initial,result=result,registry=registry,state=state,confirmed=sorted(obj._plugins_confirmados),calls=calls))
Path(__file__).with_name('phase-types.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_phase_types.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
