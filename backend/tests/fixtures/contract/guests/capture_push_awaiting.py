"""Executa o produtor e seus auxiliares com pane/ask e entrega controlados, sem sessão viva."""
import ast,json,logging,subprocess,sys,types
from pathlib import Path
from types import SimpleNamespace
SOURCE_COMMIT='9d7d1a051'
source=Path('app/api.py')
source_text=subprocess.check_output(['git','show',SOURCE_COMMIT+':backend/app/api.py'],text=True)
code=compile(ast.Module(body=[n for n in ast.parse(source_text).body if isinstance(n,ast.FunctionDef) and n.name in {'_do_notify_awaiting','_awaiting_body','_pane_wants_input'}],type_ignores=[]),str(source),'exec')
rows=[]
for provider in ['pi','omp','kimi']:
 for scenario in ['menu','overlay','capture_error','idle','late_menu','late_question','question','empty_question']:
  notices=[];sleeps=[];captures=[];reads=[]
  def capture(name):
   captures.append(name)
   if scenario=='capture_error':raise OSError('fixture')
   if scenario=='late_menu':return 'menu' if sleeps else 'idle'
   return scenario
  def classify(pane):return ('awaiting_input' if pane=='menu' else 'idle',None,'pergunta do pane' if pane=='menu' else None,None)
  app=types.ModuleType('app');app.tmux=SimpleNamespace(capture_pane=capture)
  state=types.ModuleType('app.state');state.classify=classify;state.is_overlay=lambda pane:pane=='overlay'
  sys.modules['app']=app;sys.modules['app.state']=state
  def question(path):
   reads.append(path)
   if scenario=='question' or scenario=='late_question' and sleeps:return SimpleNamespace(questions=[SimpleNamespace(question='pergunta do sidecar')])
   if scenario=='empty_question':return SimpleNamespace(questions=[])
   return None
  info=SimpleNamespace(name='fixture',jsonl='fixture.jsonl',provider=provider,headless=False)
  ns={'_log':logging.getLogger('fixture'),'registry':SimpleNamespace(list=lambda:[info]),'session_key':lambda _:'sid','_rust_conduz':lambda _:False,'read_pending_askq':question,'time':SimpleNamespace(sleep=sleeps.append),'_AWAITING_PUSH_RETRY_S':1.5,'push':SimpleNamespace(notify_awaiting=lambda *args:notices.append(list(args)))}
  exec(code,ns);ns['_do_notify_awaiting']('sid')
  rows.append({'provider':provider,'scenario':scenario,'notices':notices,'sleeps':sleeps,'captures':len(captures),'question_reads':len(reads)})
(Path(__file__).resolve().parent/'push-awaiting.json').write_text(json.dumps({'source_commit':SOURCE_COMMIT,'isolation':'AST original de produtor e auxiliares; classificador/pane/sidecar/relógio/entrega controlados. Não prova o classificador nem sessão real.','rows':rows},ensure_ascii=True,indent=2)+'\n')
print(len(rows))
