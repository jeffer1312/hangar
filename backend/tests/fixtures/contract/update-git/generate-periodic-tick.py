"""Captura um tique original com relógio e efeitos injetados, sem rede ou processos."""
import argparse,ast,asyncio,json,subprocess,types
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--reference',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
source=a.reference/'backend/app/api.py';tree=ast.parse(source.read_text());node=next(n for n in tree.body if isinstance(n,ast.AsyncFunctionDef) and n.name=='_auto_update_loop')
cases=[]
for name in ['package','disabled','normal','fetch-failed','fetch-error','reason-error','up-to-date','skipped','working','list-error','refused','launch-error']:
 events=[]
 def event(kind,*args):events.append([kind,*args])
 class Log:
  def info(self,*args):event('info',*args)
  def warning(self,*args):event('warning',*args)
  def exception(self,*args):event('exception',*args)
 def git(*args,**kwargs):
  event('git',list(args),kwargs)
  if name=='fetch-error':raise RuntimeError('fixture')
  return types.SimpleNamespace(returncode=1 if name=='fetch-failed' else 0)
 def reason():
  event('reason')
  if name=='reason-error':raise RuntimeError('fixture')
  return {'up-to-date':'em dia','skipped':'arvore suja'}.get(name)
 async def listed():
  event('list')
  if name=='list-error':raise RuntimeError('fixture')
  return [types.SimpleNamespace(state='working' if name=='working' else 'idle')]
 def start(*args,**kwargs):
  event('start',list(args),kwargs)
  if name=='launch-error':raise RuntimeError('fixture')
  return {'ok':name!='refused','erro':'fixture-refused'}
 sleeps=0
 async def sleep(delay):
  global sleeps
  event('sleep',delay);sleeps+=1
  if sleeps==2:raise asyncio.CancelledError()
 async def thread(fn,*args,**kwargs):return fn(*args,**kwargs)
 ns={'diag':types.SimpleNamespace(install_manifest=lambda: {} if name=='package' else None),'asyncio':types.SimpleNamespace(sleep=sleep,to_thread=thread),'_AUTO_UPDATE_INTERVALO':3600,'automations_enabled':lambda:name!='disabled','atualizar':types.SimpleNamespace(_git=git,_cauda=lambda p:'fixture-tail',iniciar=start),'_auto_update_motivo':reason,'registry':types.SimpleNamespace(list_with_state=listed),'settings':types.SimpleNamespace(port=8765),'_log':Log()}
 exec(compile(ast.Module(body=[node],type_ignores=[]),'<original-auto-loop>','exec'),ns)
 try:asyncio.run(ns['_auto_update_loop']())
 except asyncio.CancelledError:pass
 cases.append({'name':name,'events':events})
a.output.write_text(json.dumps({'source_commit':subprocess.check_output(['git','-C',str(a.reference),'rev-parse','HEAD'],text=True).strip(),'command':'python generate-periodic-tick.py --reference <checkout-Python> --output <fixture>','scope':'Um tique; sleep termina com CancelledError; efeitos originais substituídos por gravadores.','cases':cases},ensure_ascii=False,indent=2)+'\n');print(len(cases))
