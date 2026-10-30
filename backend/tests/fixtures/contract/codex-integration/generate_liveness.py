"""Captura consulta de processo sem sinal de encerramento e com permissão negada."""
import ast, errno, json, os, subprocess, sys
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
source=subprocess.check_output(['git','show',f'{COMMIT}:backend/app/procinfo.py'],cwd=ROOT,text=True)
node=next(node for node in ast.parse(source).body if isinstance(node,ast.FunctionDef) and node.name=='pid_vivo')
ns={};exec(compile(ast.Module(body=[node],type_ignores=[]),'backend/app/procinfo.py','exec'),ns)
cases=[]
for pid in [-1,0,1,42]:
 for result in ['ok','missing','denied','failure']:
  calls=[]
  def probe(pid,signal):
   calls.append([pid,signal])
   if result=='missing':raise ProcessLookupError(errno.ESRCH,'fixture')
   if result=='denied':raise PermissionError(errno.EPERM,'fixture')
   if result=='failure':raise OSError(errno.EIO,'fixture')
  ns.update(_TEM_PROC=True,os=SimpleNamespace(kill=probe))
  try: expected=ns['pid_vivo'](pid);error=None
  except OSError as exc:expected=None;error=dict(error=type(exc).__name__,errno=exc.errno)
  cases.append(dict(pid=pid,probe=result,expected=expected,error=error,calls=calls))
ns.update(_TEM_PROC=True,os=os)
child=subprocess.Popen([sys.executable,'-c',"import sys; print('ready',flush=True); sys.stdin.read()"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
try:
 assert child.stdout.readline().strip()=='ready'
 alive=ns['pid_vivo'](child.pid)
 child.stdin.close();child.wait(timeout=5)
 dead=ns['pid_vivo'](child.pid)
finally:
 if child.poll() is None:child.kill();child.wait()
try:ns['pid_vivo'](1<<100);overflow=None
except Exception as error:overflow=dict(error=type(error).__name__,message=str(error))
Path(__file__).with_name('liveness.json').write_text(json.dumps(dict(source_commit=COMMIT,
 command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_liveness.py',
 cases=cases,child=dict(alive=alive,dead=dead),overflow=overflow),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
