"""Captura valores persistidos no produtor original, com dependências sem efeitos."""
import ast,json,logging,re,subprocess,threading
from pathlib import Path
from types import SimpleNamespace
SOURCE_COMMIT='9d7d1a051'
source=Path('app/api.py');tree=ast.parse(subprocess.check_output(['git','show',SOURCE_COMMIT+':backend/app/api.py'],text=True))
code=compile(ast.Module(body=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in {'_terminou_se_parou','_resumo_resposta'}],type_ignores=[]),str(source),'exec')
rows=[]
for key in ['notify_finished','finish_min_seconds']:
 for value in [None,False,True,0,1,45,101,'','false','45',[],[1],{}, {'x':1},float('nan'),float('inf')]:
  notices=[];series={'sid':1000.0};settings={'notify_finished':True,'finish_min_seconds':45};settings[key]=value
  ns={'re':re,'_log':logging.getLogger('fixture'),'time':SimpleNamespace(sleep=lambda _:None),'_FINISH_SETTLE':20.0,'_turno_lock':threading.Lock(),'_working_started':series,'hook_state':SimpleNamespace(get_state=lambda _:('idle',1100.0)),'registry':SimpleNamespace(list=lambda:[SimpleNamespace(name='fixture',jsonl='fixture',provider='pi')]),'session_key':lambda _:'sid','runtime_config':SimpleNamespace(get=settings.get),'push':SimpleNamespace(notify_finished=lambda *args:notices.append(list(args))),'_notify_async':lambda sid,fn:fn('fixture')}
  exec(code,ns);error=None
  try:ns['_terminou_se_parou']('sid',1000.0,1100.0)
  except Exception as exc:error=type(exc).__name__
  rows.append({'key':key,'value':value,'notices':notices,'series':series,'error':error})
(Path(__file__).resolve().parent/'push-settings.json').write_text(json.dumps({'source_commit':SOURCE_COMMIT,'isolation':'AST do produtor; valores de runtime_config.get injetados; relógio/lista/entrega simulados, não prova o leitor do arquivo.','rows':rows},ensure_ascii=True,indent=2)+'\n')
print(len(rows))
