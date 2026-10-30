"""Executa a cadeia original de idle do Kimi até o aviso, com fontes e relógio isolados."""
import ast,json,logging,re,subprocess,threading
from pathlib import Path
from types import SimpleNamespace
SOURCE_COMMIT='9d7d1a051'
raw=subprocess.check_output(['git','show',SOURCE_COMMIT+':backend/app/api.py'],text=True)
names={'_on_hook_transition','_push_terminou','_terminou_se_parou','_resumo_resposta'}
code=compile(ast.Module(body=[node for node in ast.parse(raw).body if isinstance(node,ast.FunctionDef) and node.name in names],type_ignores=[]),'app/api.py','exec')
rows=[]
for repeats in [0,1,2]:
 for advance in [False,True]:
  notices=[];sleeps=[];marker=['idle',1100.0];remaining=[repeats];series={'sid':1000.0}
  class Thread:
   def __init__(self,target,daemon): self.target=target
   def start(self): self.target()
  class Timer:
   def __init__(self,interval,function,args):self.interval=interval;self.function=function;self.args=args
   def start(self):
    sleeps.append(self.interval);remaining[0]-=1
    if advance:marker[1]+=1
    self.function(*self.args)
  info=SimpleNamespace(name='session',jsonl='fixture.jsonl',provider='kimi')
  ns={'re':re,'_log':logging.getLogger('fixture'),'time':SimpleNamespace(sleep=sleeps.append),'threading':SimpleNamespace(Thread=Thread,Timer=Timer),'_FINISH_SETTLE':20.0,'_RECHECA_KIMI':5.0,'_CONFIRM_GRACE':8.0,'_turno_lock':threading.Lock(),'_working_started':series,'_last_idle':{},'_falhas_seguidas':{},'hook_state':SimpleNamespace(get_state=lambda _:tuple(marker)),'registry':SimpleNamespace(list=lambda:[info]),'session_key':lambda _:'sid','_rust_conduz':lambda _:False,'runtime_config':SimpleNamespace(get=lambda key:True if key=='notify_finished' else 45),'push':SimpleNamespace(notify_finished=lambda *args:notices.append(list(args))),'_notify_async':lambda sid,fn:fn('session'),'corrige_ocioso_kimi':lambda m,path:('working',m[1]) if remaining[0] else m,'_drenar':lambda *args:0,'_agendar_confirmacao':lambda *args:None,'loop_mod':SimpleNamespace(LoopLink=lambda name:SimpleNamespace(get=lambda:None),ACTIVE=[]),'_maybe_chain':lambda *args:None,'_armar_recheca':lambda *args:True}
  exec(code,ns);ns['_recheca_kimi']=ns['_on_hook_transition'];ns['_on_hook_transition']('sid','idle')
  rows.append({'rechecks':repeats,'advance_marker':advance,'notices':notices,'sleeps':sleeps,'series':series})
Path(__file__).with_name('push-kimi-finish.json').write_text(json.dumps({'source_commit':SOURCE_COMMIT,'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push_kimi_finish.py','isolation':'AST dos quatro produtores; correção de idle, fila, lista, entrega e relógio substituídos. Não prova parser wire nem multiplexador.','rows':rows},ensure_ascii=False,indent=2)+'\n')
print(len(rows))
