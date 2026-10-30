"""Captura o reparo do intérprete legado sem tocar na instalação da máquina."""
import ast, copy, json, os, shlex, subprocess, tempfile
from pathlib import Path
from types import SimpleNamespace
ROOT=Path(__file__).resolve().parents[5]
COMMIT='8a3ce81c1'
def extract(path,names,namespace):
 source=subprocess.check_output(['git','show',f'{COMMIT}:{path}'],cwd=ROOT,text=True)
 nodes=[n for n in ast.parse(source).body if isinstance(n,ast.FunctionDef) and n.name in names]
 exec(compile(ast.Module(body=nodes,type_ignores=[]),path,'exec'),namespace)
cases=[]
for windows in [False,True]:
 for legacy_exists in [False,True]:
  for current_exists in [False,True]:
   for form in ['quoted','plain','powershell','escaped','other-python','other-hook','empty','broken','multiple','invalid-groups','invalid-hook','duplicate-events']:
    with tempfile.TemporaryDirectory() as raw:
     root=Path(raw);repo=root/'repo';scripts=repo/'backend/hooks';scripts.mkdir(parents=True)
     current=root/'python';legacy=repo/'backend/.venv'/('Scripts/python.exe' if windows else 'bin/python')
     if current_exists:current.touch()
     if legacy_exists:legacy.parent.mkdir(parents=True);legacy.touch()
     state=str(scripts/'state_hook.py');nav=str(scripts/'nav_hook.py');guard=str(scripts/'guard_tmux.py')
     ns=dict(Path=Path,os=SimpleNamespace(name='nt' if windows else 'posix',path=os.path),shlex=shlex,
      STATE_HOOK=state,NAV_HOOK=nav,GUARD_HOOK=guard,_PYTHON=str(current),
      _STATE_COMMAND=f'"{current}" "{state}" || exit 0',_NAV_COMMAND=f'"{current}" "{nav}" || exit 0')
     extract('backend/app/hook_installer.py',{'_tokens','_refers_to'},ns)
     extract('backend/app/codex_hook_installer.py',{'comando_estado','comando_navegador','comando_guarda','_reparar_python_legado'},ns)
     command=f'"{legacy}" "{state}" || exit 0'
     if form=='plain':command=f'{legacy} {state}'
     if form=='powershell':command='& '+command
     if form=='escaped':command=f'{legacy} {state}'.replace('/repo/','/re\\po/')
     if form=='other-python':command=f'python3 "{state}"'
     if form=='other-hook':command=f'"{legacy}" /custom/personal.py'
     if form=='empty':command=''
     if form=='broken':command=f'"{legacy} "{state}"'
     hooks={'Stop':[{'hooks':[{'type':'command','command':command,'timeout':9}]}]}
     if form=='multiple':hooks['Stop'][0]['hooks'] += [{'command':f'"{legacy}" "{nav}"'},{'command':f'"{legacy}" "{guard}"'}]
     if form=='invalid-groups':hooks['Other']=42;hooks['Stop'] += [None,{'hooks':4}]
     if form=='invalid-hook':hooks['Stop'][0]['hooks'] += [None,{'command':False}]
     if form=='duplicate-events':hooks['Start']=copy.deepcopy(hooks['Stop'])
     before=copy.deepcopy(hooks)
     try:result={'ok':ns['_reparar_python_legado'](hooks,windows)}
     except Exception as error:result={'error':type(error).__name__,'message':str(error)}
     case=dict(windows=windows,legacy_exists=legacy_exists,current_exists=current_exists,form=form,before=before,after=hooks,result=result)
     cases.append(json.loads(json.dumps(case).replace(raw,'$ROOT')))
Path(__file__).with_name('legacy-hooks.json').write_text(json.dumps(dict(source_commit=COMMIT,command='cd backend && uv run python tests/fixtures/contract/codex-integration/generate_legacy_hooks.py',cases=cases),ensure_ascii=True,indent=2)+'\n')
print(len(cases))
