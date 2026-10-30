"""Executa a transição original de morte com lista, entrega e threads isoladas."""
import ast,json,subprocess,threading
from pathlib import Path
from types import SimpleNamespace
SOURCE_COMMIT='9d7d1a051'
raw=subprocess.check_output(['git','show',SOURCE_COMMIT+':backend/app/api.py'],text=True)
code=compile(ast.Module(body=[node for node in ast.parse(raw).body if isinstance(node,ast.FunctionDef) and node.name=='_on_hook_transition'],type_ignores=[]),'app/api.py','exec')
rows=[]
for enabled in [None,False,True,0,1,'','false',[],[1],{}, {'x':1}]:
 for present in [False,True]:
  notices=[];started={'sid':1000.0,'other':900.0};idle={'sid':1100.0,'other':901.0}
  class Thread:
   def __init__(self,target,daemon): self.target=target
   def start(self): self.target()
  def notify(sid,send):
   if present: send('fixture')
  ns={'_working_started':started,'_last_idle':idle,'_turno_lock':threading.Lock(),'runtime_config':SimpleNamespace(get=lambda _:enabled),'_notify_async':notify,'push':SimpleNamespace(notify_dead=lambda name:notices.append(name)),'threading':SimpleNamespace(Thread=Thread),'registry':SimpleNamespace(list=lambda:[])}
  exec(code,ns);ns['_on_hook_transition']('sid','dead')
  rows.append({'enabled':enabled,'present':present,'notices':notices,'started':started,'last_idle':idle})
Path(__file__).with_name('push-dead.json').write_text(json.dumps({'source_commit':SOURCE_COMMIT,'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push_dead.py','isolation':'Callback original; lista vazia no trabalho independente; resolução e envio substituídos. Não prova rede nem descoberta de processos.','rows':rows},ensure_ascii=False,indent=2)+'\n')
print(len(rows))
