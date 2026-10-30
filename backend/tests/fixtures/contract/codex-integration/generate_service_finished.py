"""Captura espera da rodada já terminada com sincronização desligada."""
import ast,asyncio,json,subprocess,types
from pathlib import Path
ROOT=Path(__file__).resolve().parents[5];COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True)
cls=next(n for n in ast.parse(source).body if isinstance(n,ast.ClassDef) and n.name=='IntegracaoCodex')
method=next(n for n in cls.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='atualizar_e_aguardar')
ns={'asyncio':asyncio};exec(compile(ast.Module(body=[method],type_ignores=[]),'original-finished','exec'),ns)
async def run(kind):
 async def worker():
  if kind=='error':raise ValueError('fixture')
  if kind=='cancelled':raise asyncio.CancelledError()
 async def session():return {'estado':'erro'}
 task=asyncio.create_task(worker())
 try:await task
 except (ValueError,asyncio.CancelledError):pass
 service=types.SimpleNamespace(_task=task,_task_forcada=False,sessao=session,status=lambda:{'estado':'erro'})
 try:result={'ok':(await ns['atualizar_e_aguardar'](service,False))['estado']}
 except BaseException as error:result={'error':type(error).__name__,'message':str(error)}
 return {'kind':kind,'result':result}
cases=[asyncio.run(run(kind)) for kind in ['ok','error','cancelled']]
Path(__file__).with_name('service-finished.json').write_text(json.dumps({'source_commit':COMMIT,'command':'cd backend && uv run python tests/fixtures/contract/codex-integration/generate_service_finished.py','cases':cases},indent=2)+'\n')
print(len(cases))
