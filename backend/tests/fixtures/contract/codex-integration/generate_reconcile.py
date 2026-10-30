"""Captura a decisão original de reutilizar o preparo Codex concluído."""
import ast
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/codex_integracao.py'],cwd=ROOT,text=True)
cls=next(n for n in ast.parse(source).body if isinstance(n,ast.ClassDef) and n.name=='IntegracaoCodex')
method=next(n for n in cls.body if isinstance(n,ast.FunctionDef) and n.name=='precisa_reconciliar')
retry=next(n.value.value for n in ast.parse(source).body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='_RETENTATIVA' for t in n.targets))
ns=dict(datetime=datetime,time=SimpleNamespace(time=lambda:1000.0),_RETENTATIVA=retry)
exec(compile(ast.Module(body=[method],type_ignores=[]),'original-reconcile','exec'),ns)
cases=[]
for state in ['ok','parcial','erro','ocioso','executando','other']:
    for fingerprint in ['same','changed','error']:
        for running in [False,True]:
            for last in [None,datetime.fromtimestamp(0,timezone.utc).isoformat(),datetime.fromtimestamp(999,timezone.utc).isoformat()]:
                for next_update in [None,datetime.fromtimestamp(0,timezone.utc).isoformat(),datetime.fromtimestamp(2000,timezone.utc).isoformat()]:
                    data=dict(fingerprint='same',status=dict(estado=state,ultima_execucao=last,proxima_atualizacao=next_update))
                    ns['json_obj']=lambda path:data
                    def current():
                        if fingerprint=='error':raise OSError('fixture')
                        return fingerprint
                    obj=SimpleNamespace(_task=SimpleNamespace(done=lambda:not running),raiz=Path('/fixture/state'),fingerprint=current)
                    try:result=dict(ok=ns['precisa_reconciliar'](obj))
                    except Exception as error:result=dict(error=type(error).__name__,message=str(error))
                    cases.append(dict(state=state,fingerprint=fingerprint,running=running,last=last,next_update=next_update,data=data,result=result))
Path(__file__).with_name('reconcile.json').write_text(json.dumps(dict(source_commit=COMMIT,retry=retry,
    command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_reconcile.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(f'{len(cases)} decisões de preparo capturadas')
