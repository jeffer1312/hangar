"""Caracteriza a troca de série e a segunda descoberta do produtor original de conclusão."""
import ast,json,logging,re,subprocess,threading
from pathlib import Path
from types import SimpleNamespace
SOURCE_COMMIT='9d7d1a051'
raw=subprocess.check_output(['git','show',SOURCE_COMMIT+':backend/app/api.py'],text=True)
names={'_terminou_se_parou','_resumo_resposta','_notify_async'}
code=compile(ast.Module(body=[node for node in ast.parse(raw).body if isinstance(node,ast.FunctionDef) and node.name in names],type_ignores=[]),'app/api.py','exec')
rows=[]
for scenario in ['stable','new_series_during_list','gone_before_send','appeared_before_send']:
 notices=[];sleeps=[];series={'sid':1000.0};reads=[0]
 info=SimpleNamespace(name='session',jsonl='fixture.jsonl',provider='pi')
 def listing():
  reads[0]+=1
  if scenario=='new_series_during_list':series['sid']=999.0
  if scenario=='gone_before_send' and reads[0]>1:return []
  if scenario=='appeared_before_send' and reads[0]==1:return []
  return [info]
 class Thread:
  def __init__(self,target,daemon):self.target=target
  def start(self):self.target()
 ns={'re':re,'_log':logging.getLogger('fixture'),'time':SimpleNamespace(sleep=sleeps.append),'threading':SimpleNamespace(Thread=Thread),'_FINISH_SETTLE':20.0,'_turno_lock':threading.Lock(),'_working_started':series,'hook_state':SimpleNamespace(get_state=lambda _:('idle',1100.0)),'registry':SimpleNamespace(list=listing),'session_key':lambda _:'sid','_rust_conduz':lambda _:False,'runtime_config':SimpleNamespace(get=lambda key:True if key=='notify_finished' else 45),'push':SimpleNamespace(notify_finished=lambda *args:notices.append(list(args)))}
 exec(code,ns);ns['_terminou_se_parou']('sid',1000.0,1100.0)
 rows.append({'scenario':scenario,'notices':notices,'sleeps':sleeps,'series':series,'list_reads':reads[0]})
Path(__file__).with_name('push-finish-races.json').write_text(json.dumps({'source_commit':SOURCE_COMMIT,'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push_finish_races.py','isolation':'AST do produtor e resolvedor originais; lista muta em pontos determinísticos; threads executadas em linha, sem rede.','rows':rows},ensure_ascii=False,indent=2)+'\n')
print(len(rows))
