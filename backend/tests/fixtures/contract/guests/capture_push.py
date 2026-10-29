"""Executa os produtores originais com relógio, lista e entrega isolados; sem importar a API viva."""
import ast
import json
import logging
import re
import subprocess
import threading
from pathlib import Path
from types import SimpleNamespace
HERE=Path(__file__).resolve().parent
source=HERE.parents[3]/'app/api.py'
tree=ast.parse(source.read_text())
names={'_do_notify_awaiting','_terminou_se_parou','_resumo_resposta'}
selected=[node for node in tree.body if isinstance(node,ast.FunctionDef) and node.name in names]
code=compile(ast.Module(body=selected,type_ignores=[]),str(source),'exec')
rows=[]
for provider in ['pi','omp','kimi']:
    for scenario in ['real','late_pane','late_question','idle','absent']:
        notices=[];sleeps=[];calls=[0]
        info=SimpleNamespace(name='session',jsonl='fixture.jsonl',provider=provider,headless=False)
        def question(_):
            return SimpleNamespace(questions=[SimpleNamespace(question='pergunta')]) if scenario=='late_question' and sleeps else None
        def pane(_):
            calls[0]+=1
            return scenario=='real' or scenario=='late_pane' and calls[0]>1
        ns={'_log':logging.getLogger('fixture'),'registry':SimpleNamespace(list=lambda:[] if scenario=='absent' else [info]),
            'session_key':lambda _: 'sid','_rust_conduz':lambda _:False,'read_pending_askq':question,
            '_pane_wants_input':pane,'_awaiting_body':lambda _:'pergunta','time':SimpleNamespace(sleep=sleeps.append),
            '_AWAITING_PUSH_RETRY_S':1.5,'push':SimpleNamespace(notify_awaiting=lambda *args:notices.append(list(args)))}
        exec(code,ns);ns['_do_notify_awaiting']('sid')
        rows.append({'provider':provider,'scenario':scenario,'notices':notices,'sleeps':sleeps,'pane_reads':calls[0]})
finished=[]
for provider in ['pi','omp','kimi']:
    for scenario in ['long','short','disabled','working','new_idle','new_series']:
        notices=[];sleeps=[];started=1000.0;idle=1010.0 if scenario=='short' else 1100.0
        series={'sid':999.0 if scenario=='new_series' else started}
        marker=('working' if scenario=='working' else 'idle',idle+1 if scenario=='new_idle' else idle)
        info=SimpleNamespace(name='session',jsonl='fixture.jsonl',provider=provider)
        ns={'re':re,'_log':logging.getLogger('fixture'),'time':SimpleNamespace(sleep=sleeps.append),'_FINISH_SETTLE':20.0,
            '_turno_lock':threading.Lock(),'_working_started':series,'hook_state':SimpleNamespace(get_state=lambda _:marker),
            'registry':SimpleNamespace(list=lambda:[info]),'session_key':lambda _:'sid',
            'runtime_config':SimpleNamespace(get=lambda key:scenario!='disabled' if key=='notify_finished' else 45),
            'push':SimpleNamespace(notify_finished=lambda *args:notices.append(list(args))),
            '_notify_async':lambda sid,fn:fn('session')}
        exec(code,ns);ns['_terminou_se_parou']('sid',started,idle)
        finished.append({'provider':provider,'scenario':scenario,'notices':notices,'sleeps':sleeps,'series':series})
result={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'command':'cd backend && uv run python tests/fixtures/contract/guests/capture_push.py','awaiting':rows,'finished':finished}
(HERE/'push.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print(f'{len(rows)} decisões aguardando e {len(finished)} decisões terminou capturadas')
