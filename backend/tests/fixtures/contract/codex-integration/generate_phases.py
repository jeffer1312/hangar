"""Captura cache, invalidação e publicação dos checkpoints por fase."""
import ast, asyncio, json, subprocess
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True)
cls=next(node for node in ast.parse(source).body if isinstance(node,ast.ClassDef) and node.name=='IntegracaoCodex')
method=next(node for node in cls.body if isinstance(node,ast.AsyncFunctionDef) and node.name=='_run_phase')
ns=dict(asyncio=asyncio,time=SimpleNamespace(time=lambda:1000.0))
messages=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_msgs.py'],cwd=ROOT,text=True)
exec(compile(messages,'backend/app/codex_msgs.py','exec'),ns)
exec(compile(ast.Module(body=[method],type_ignores=[]),'backend/app/codex_integracao.py','exec'),ns)
cases=[]
for phase in ['instructions','plugins','fragments','skills']:
 for force in [False,True]:
  for previous in ['absent','same','different']:
   for outcome in ['ok','source-changed','error']:
    registry={'phases':{}}
    if previous!='absent':registry['phases'][phase]=dict(fingerprint='same' if previous=='same' else 'other',errors=['old error'],warnings=['old warning'],confirmed=['z','a'],plugins=['old plugin'],next_update='old date',extra='discard')
    state=dict(erros=['existing error'],avisos=['existing warning'],plugins=[],proxima_atualizacao=None)
    calls=[];source_reads=0
    def fingerprint(**kwargs):
     global source_reads
     calls.append(['fingerprint',kwargs.get('fontes',False)])
     if kwargs.get('fontes'):
      source_reads+=1
      return {phase:'changed' if outcome=='source-changed' and source_reads>1 else 'source'}
     return {phase:'same'}
    obj=SimpleNamespace(_estado=state,_plugins_confirmados=set(),_phase_fingerprints=fingerprint)
    async def action():
     calls.append(['action']);state['erros'].append('new error');state['avisos'].append('new warning')
     if phase=='plugins':obj._plugins_confirmados={'b','a'};state['plugins']=['new plugin'];state['proxima_atualizacao']='new date'
     if outcome=='error':raise RuntimeError('fixture action')
    initial=json.loads(json.dumps(registry))
    try:asyncio.run(ns['_run_phase'](obj,phase,registry,action,force=force));result={'ok':None}
    except Exception as error:result={'error':type(error).__name__,'message':str(error)}
    cases.append(dict(phase=phase,force=force,previous=previous,outcome=outcome,initial=initial,
       result=result,registry=registry,state=state,confirmed=sorted(obj._plugins_confirmados),calls=calls))
Path(__file__).with_name('phases.json').write_text(json.dumps(dict(source_commit=COMMIT,
 command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_phases.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
