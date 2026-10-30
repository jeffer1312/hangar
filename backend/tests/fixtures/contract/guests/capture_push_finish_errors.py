"""Executa falhas de descoberta do produtor original sem consultar sessões reais."""
import ast,json,logging,re,subprocess,threading
from pathlib import Path
from types import SimpleNamespace
SOURCE_COMMIT='9d7d1a051'
raw=subprocess.check_output(['git','show',SOURCE_COMMIT+':backend/app/api.py'],text=True)
names={'_terminou_se_parou','_resumo_resposta','_notify_async'}
code=compile(ast.Module(body=[node for node in ast.parse(raw).body if isinstance(node,ast.FunctionDef) and node.name in names],type_ignores=[]),'app/api.py','exec')
rows=[]
for failed_read in [1,2]:
 notices=[];series={'sid':1000.0};reads=[0]
 info=SimpleNamespace(name='session',jsonl='fixture.jsonl',provider='pi')
 def listing():
  reads[0]+=1
  if reads[0]==failed_read:raise RuntimeError('fixture_list_failed')
  return [info]
 class Thread:
  def __init__(self,target,daemon):self.target=target
  def start(self):self.target()
 ns={'re':re,'_log':logging.getLogger('fixture'),'time':SimpleNamespace(sleep=lambda _:None),'threading':SimpleNamespace(Thread=Thread),'_FINISH_SETTLE':20.0,'_turno_lock':threading.Lock(),'_working_started':series,'hook_state':SimpleNamespace(get_state=lambda _:('idle',1100.0)),'registry':SimpleNamespace(list=listing),'session_key':lambda _:'sid','_rust_conduz':lambda _:False,'runtime_config':SimpleNamespace(get=lambda key:True if key=='notify_finished' else 45),'push':SimpleNamespace(notify_finished=lambda *args:notices.append(list(args)))}
 exec(code,ns);error=None
 try:ns['_terminou_se_parou']('sid',1000.0,1100.0)
 except Exception as exc:error=type(exc).__name__
 rows.append({'failed_read':failed_read,'notices':notices,'series':series,'list_reads':reads[0],'error':error})
Path(__file__).with_name('push-finish-errors.json').write_text(json.dumps({'source_commit':SOURCE_COMMIT,'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push_finish_errors.py','isolation':'Produtor e resolvedor originais em AST; falha injetada na consulta da lista; threads em linha, sem rede.','rows':rows},ensure_ascii=False,indent=2)+'\n')
print(len(rows))
